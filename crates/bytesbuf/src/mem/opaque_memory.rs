// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use alloc::boxed::Box;
use core::any::{Any, TypeId};

use thread_aware::ThreadAware;

use crate::mem::{Memory, MemoryShared};

/// Adapter to erase the type of a [`MemoryShared`] implementation.
///
/// This adapter adds some inefficiency due to additional indirection overhead for
/// every memory reservation, so avoid this adapter if you can tolerate alternatives (generics).
///
/// The adapter is itself [`MemoryShared`]. It owns the wrapped provider and forwards [`ThreadAware`]
/// relocation to it, leaving the decision of how to be thread-aware entirely with the wrapped
/// provider. Cloning the adapter clones the wrapped provider; whether the clones then share any
/// state is up to that provider.
#[derive(Debug, ThreadAware)]
pub struct OpaqueMemory {
    inner: Box<dyn MemoryShared>,
}

impl OpaqueMemory {
    /// Creates a new instance of the adapter.
    ///
    /// # Panics
    ///
    /// Panics only if runtime type identification reports [`OpaqueMemory`] but the downcast of
    /// the same value to [`OpaqueMemory`] fails, which would indicate a standard library defect.
    #[must_use]
    pub fn new<M: MemoryShared>(inner: M) -> Self {
        if TypeId::of::<M>() == TypeId::of::<Self>() {
            let inner: Box<dyn Any> = Box::new(inner);
            *inner
                .downcast::<Self>()
                .expect("the concrete type was verified as OpaqueMemory above")
        } else {
            Self { inner: Box::new(inner) }
        }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`][1] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// If this method returns, the buffer has at least `min_bytes` bytes of capacity.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`][1]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// The wrapped provider may panic or abort if the requested capacity cannot be obtained.
    ///
    /// [1]: crate::BytesBuf
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.inner.reserve(min_bytes)
    }
}

impl Clone for OpaqueMemory {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_boxed(),
        }
    }
}

impl Memory for OpaqueMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, feature = "std"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{self, AtomicUsize};

    use static_assertions::assert_impl_all;
    use thread_aware::{Relocator, Thread};

    use super::*;
    use crate::mem::GlobalPool;

    assert_impl_all!(OpaqueMemory: MemoryShared, ThreadAware);

    #[test]
    fn wraps_inner() {
        let provider = GlobalPool::new();
        let memory = OpaqueMemory::new(provider);

        let builder = memory.reserve(1024);
        assert!(builder.capacity() >= 1024);
    }

    #[test]
    fn accepts_existing_opaque_memory() {
        let memory = OpaqueMemory::new(GlobalPool::new());
        let memory = OpaqueMemory::new(memory);

        let builder = memory.reserve(1024);

        assert!(builder.capacity() >= 1024);
    }

    #[test]
    fn memory_trait() {
        let provider = GlobalPool::new();
        let memory = OpaqueMemory::new(provider);

        // Call reserve via the Memory trait to verify the impl block
        let builder = Memory::reserve(&memory, 1024);
        assert!(builder.capacity() >= 1024);
    }

    #[test]
    fn relocate_forwards_to_wrapped_provider() {
        // A provider whose relocate is observable, to verify forwarding.
        #[derive(Clone, Debug)]
        struct TrackingMemory {
            relocated: Arc<AtomicUsize>,
            inner: GlobalPool,
        }

        impl Memory for TrackingMemory {
            fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
                self.inner.reserve(min_bytes)
            }
        }

        impl ThreadAware for TrackingMemory {
            fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
                self.relocated.fetch_add(1, atomic::Ordering::SeqCst);
                self.inner.relocate(source, destination);
            }
        }

        let relocated = Arc::new(AtomicUsize::new(0));
        let mut memory = OpaqueMemory::new(TrackingMemory {
            relocated: Arc::clone(&relocated),
            inner: GlobalPool::new(),
        });

        _ = Relocator::between_threads().relocate(&mut memory);

        assert_eq!(relocated.load(atomic::Ordering::SeqCst), 1);
    }
}
