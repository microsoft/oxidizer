// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display};
use std::ops::{Deref, DerefMut};

/// A promise that a T is thread-safe (`Send` and `Sync`), even if the type `T` is not originally
/// so. This may be because `T` is over-generalized and not all instances are thread-safe, or
/// because it is used in special circumstances where its typical thread-safety guarantees can be
/// strengthened by other factors.
#[derive(Copy, Clone, derive_more::Debug, Default, Eq, Hash, Ord, PartialOrd, PartialEq)]
#[debug("{inner:?}")]
#[repr(transparent)]
pub struct ThreadSafe<T> {
    inner: T,
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "this is escape hatch used internally in this crate, the callers are responsible for ensuring that the inner type is correctly used"
)]
// SAFETY: Forwarding the guarantees received in new().
unsafe impl<T> Send for ThreadSafe<T> {}

// SAFETY: Forwarding the guarantees received in new().
unsafe impl<T> Sync for ThreadSafe<T> {}

impl<T> ThreadSafe<T> {
    /// # Safety
    ///
    /// The caller must ensure that the inner value truly is thread-safe,
    /// both for sending and for referencing (`Send` and `Sync`).
    pub const unsafe fn new(inner: T) -> Self {
        Self { inner }
    }

    // Currently only used in tests, so cfg-gated, but we can remove the gate if we need to use it outside tests in the future.
    #[cfg(test)]
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> Deref for ThreadSafe<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for ThreadSafe<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: Display> Display for ThreadSafe<T> {
    #[cfg_attr(test, mutants::skip)] // No API contract.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::{ptr, thread};

    use negative_impl::negative_impl;

    use super::*;

    #[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
    struct NotSafeNotSync {
        value: u64,
    }

    #[negative_impl]
    impl !Send for NotSafeNotSync {}
    #[negative_impl]
    impl !Sync for NotSafeNotSync {}

    #[test]
    fn smoke_test() {
        let not_safe = NotSafeNotSync { value: 1234 };

        // SAFETY: We must promise it really is thread safe. Sure, why not.
        let pretend_safe = unsafe { ThreadSafe::new(not_safe) };

        thread::spawn(move || {
            let wrapper = pretend_safe;

            assert_eq!(wrapper.value, 1234);

            let inner = wrapper.into_inner();

            assert_eq!(inner.value, 1234);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn derived_traits() {
        let not_safe = NotSafeNotSync { value: 1234 };

        // SAFETY: We must promise it really is thread safe. Sure, why not.
        let pretend_safe = unsafe { ThreadSafe::new(not_safe) };

        let mut clone = pretend_safe.clone();
        assert_eq!(pretend_safe, clone);
        clone.value = 4321;
        assert_ne!(pretend_safe, clone);

        assert!(pretend_safe < clone);
        assert!(clone > pretend_safe);

        assert!(pretend_safe <= clone);
        assert!(clone >= pretend_safe);

        assert!(pretend_safe <= pretend_safe);
        assert!(pretend_safe >= pretend_safe);

        assert_eq!(pretend_safe, pretend_safe);

        let default = ThreadSafe::<NotSafeNotSync>::default();
        assert_eq!(default.value, 0);
    }

    struct Arbitrary {
        thing: Vec<*const u64>,
    }

    #[test]
    fn lacking_derived_traits() {
        let value = Arbitrary {
            thing: vec![ptr::dangling()],
        };

        // SAFETY: We must promise it really is thread safe. Sure, why not.
        let pretend_safe = unsafe { ThreadSafe::new(value) };

        let inner = pretend_safe.into_inner();

        assert_eq!(inner.thing.len(), 1);
    }
}