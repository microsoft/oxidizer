// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use crate::mem::{Memory, MemoryShared};

/// Adapter to erase the type of a [`MemoryShared`] implementation.
///
/// This adapter adds some inefficiency due to additional indirection overhead for
/// every memory reservation, so avoid this adapter if you can tolerate alternatives (generics).
///
/// The adapter is itself [`MemoryShared`]. It owns the wrapped provider and forwards [`ThreadAware`]
/// relocation to it, leaving the decision of how to be thread-aware entirely with the wrapped
/// provider. Cloning the adapter clones the wrapped provider, so each clone is independent.
#[derive(Debug, ThreadAware)]
pub struct OpaqueMemory {
    inner: Box<dyn MemoryShared>,
}

impl OpaqueMemory {
    /// Creates a new instance of the adapter.
    #[must_use]
    pub fn new(inner: impl MemoryShared) -> Self {
        Self { inner: Box::new(inner) }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`][1] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`][1]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
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
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{self, AtomicUsize};

    use static_assertions::assert_impl_all;
    use thread_aware::affinity::{Affinity, pinned_affinities};

    use super::*;
    use crate::mem::GlobalPool;

    assert_impl_all!(OpaqueMemory: MemoryShared);

    #[test]
    fn wraps_inner() {
        let provider = GlobalPool::new();
        let memory = OpaqueMemory::new(provider);

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
    fn relocate_does_not_break_reservation() {
        let mut memory = OpaqueMemory::new(GlobalPool::new());

        let affinities = pinned_affinities(&[2]);
        memory.relocate(Some(affinities[0]), affinities[1]);

        // The adapter must remain usable after relocation.
        let builder = memory.reserve(1024);
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
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
                self.relocated.fetch_add(1, atomic::Ordering::SeqCst);
            }
        }

        let relocated = Arc::new(AtomicUsize::new(0));
        let mut memory = OpaqueMemory::new(TrackingMemory {
            relocated: Arc::clone(&relocated),
            inner: GlobalPool::new(),
        });

        let affinities = pinned_affinities(&[2]);
        memory.relocate(Some(affinities[0]), affinities[1]);

        assert_eq!(relocated.load(atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn clone_is_usable_independently() {
        let memory = OpaqueMemory::new(GlobalPool::new());
        let mut clone = memory.clone();

        // Relocating the clone must leave both the clone and the original usable, since each owns
        // its own wrapped provider.
        let affinities = pinned_affinities(&[2]);
        clone.relocate(Some(affinities[0]), affinities[1]);

        assert!(memory.reserve(64).capacity() >= 64);
        assert!(clone.reserve(64).capacity() >= 64);
    }
}
