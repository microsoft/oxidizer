// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Tracks all bound primitives of an I/O driver and allows their cleanup to be observed.
///
/// An I/O driver is given a `Runtime` as an argument at creation time. This is used, among other
/// things, to release platform resources owned by primitives that are dropped. However, the runtime
/// is only really guaranteed to operate while the I/O driver is active. Once the I/O driver is
/// dropped, there is no guarantee that anything keeps the runtime alive (as it is not typical to
/// wait for a runtime to complete all work - if it is told to stop, it will just abandon work).
///
/// This could lead to resource leaks if the cleanup tasks are simply dropped, because a runtime
/// stop does not necessarily mean the process will be terminated (which would allow us to rely on
/// the operating system to clean up). To avoid this, we register every bound primitive in this
/// registry. The I/O driver will only exit once no more bound primitives exist (once the cleanup
/// logic of each primitive has completed).
///
/// # Thread safety
///
/// This is a single-threaded type, only meant to be accessed from the same thread as used to
/// host the I/O driver.
#[derive(Debug, Default)]
pub struct BoundPrimitiveRegistry {
    // This is a shared data set because it is not present on any hot path.
    //
    // The data is touched from the following places:
    // 1. Incremented when creating a primitive.
    // 2. Decremented when primitive cleanup finishes.
    // 3. Checked by I/O driver shutdown logic to see whether the count is zero.
    primitive_count: Arc<AtomicUsize>,
}

impl BoundPrimitiveRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primitive_count.load(Ordering::Relaxed) == 0
    }

    #[must_use]
    pub fn register(&self) -> PrimitiveRegistrationGuard {
        self.primitive_count.fetch_add(1, Ordering::Relaxed);

        PrimitiveRegistrationGuard {
            primitive_count: Arc::clone(&self.primitive_count),
        }
    }
}

/// Tracks the time span during which a primitive is bound to the I/O driver.
/// When dropped, the primitive is considered unbound.
///
/// # Thread safety
///
/// This type is thread-safe.
#[derive(Debug)]
pub struct PrimitiveRegistrationGuard {
    primitive_count: Arc<AtomicUsize>,
}

impl Drop for PrimitiveRegistrationGuard {
    fn drop(&mut self) {
        // We do not somehow "notify" the I/O driver that a cleanup has completed, to avoid cross-
        // thread chatter. Most of the time, it does not care - it only cares when shutting down,
        // in which case we just expect it to poll for "doneness" at a regular interval.
        self.primitive_count.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    #[test]
    fn bound_primitive_registry() {
        let registry = BoundPrimitiveRegistry::new();
        assert!(registry.is_empty());

        let guard = registry.register();
        assert!(!registry.is_empty());

        let guard2 = registry.register();
        assert!(!registry.is_empty());

        drop(guard);
        assert!(!registry.is_empty());

        drop(guard2);
        assert!(registry.is_empty());
    }

    #[test]
    fn registry_is_thread_safe_type() {
        // We do not strictly need to be thread-safe under every I/O model but there is no advantage
        // to not being thread-safe because the internal logic all needs to be synchronized anyway.
        assert_impl_all!(BoundPrimitiveRegistry: Send, Sync);
    }

    #[test]
    fn guard_is_thread_safe_type() {
        assert_impl_all!(PrimitiveRegistrationGuard: Send, Sync);
    }
}