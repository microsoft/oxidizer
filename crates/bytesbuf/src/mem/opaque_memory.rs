// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::{PerCore, ThreadAware};

use crate::mem::{Memory, MemoryShared};

/// Adapter to erase the type of a [`MemoryShared`] implementation.
///
/// This adapter adds some inefficiency due to additional indirection overhead for
/// every memory reservation, so avoid this adapter if you can tolerate alternatives (generics).
///
/// The adapter is itself [`MemoryShared`], forwarding [`ThreadAware`] relocation to the wrapped
/// provider so that thread-affine state is relocated correctly when the adapter moves between
/// threads.
#[derive(Clone, Debug)]
pub struct OpaqueMemory {
    inner: thread_aware::Arc<dyn MemoryShared, PerCore>,
}

impl OpaqueMemory {
    /// Creates a new instance of the adapter.
    ///
    /// The wrapped provider must be [`Clone`] so that a thread-local instance can be materialized
    /// per thread, preserving thread-awareness across relocations.
    #[must_use]
    pub fn new(inner: impl MemoryShared + Clone) -> Self {
        Self {
            inner: thread_aware::Arc::<dyn MemoryShared, PerCore>::with_clone_fn(inner, |provider| Box::new(provider.clone())),
        }
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

impl Memory for OpaqueMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

impl ThreadAware for OpaqueMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn relocate(&mut self, source: Option<thread_aware::affinity::Affinity>, destination: thread_aware::affinity::Affinity) {
        self.inner.relocate(source, destination);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

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
        use thread_aware::affinity::pinned_affinities;

        let mut memory = OpaqueMemory::new(GlobalPool::new());

        let affinities = pinned_affinities(&[2]);
        memory.relocate(Some(affinities[0]), affinities[1]);

        // The adapter must remain usable after relocation.
        let builder = memory.reserve(1024);
        assert!(builder.capacity() >= 1024);
    }
}
