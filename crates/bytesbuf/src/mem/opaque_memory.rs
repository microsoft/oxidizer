// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::mem::{Memory, MemoryShared};

/// Adapter to erase the type of a [`MemoryShared`] implementation.
///
/// This adapter adds some inefficiency due to additional indirection overhead for
/// every memory reservation, so avoid this adapter if you can tolerate alternatives (generics).
#[derive(Clone, Debug)]
pub struct OpaqueMemory {
    inner: Arc<dyn Memory + Send + Sync + 'static>,
}

impl OpaqueMemory {
    /// Creates a new instance of the adapter.
    #[must_use]
    pub fn new(inner: impl MemoryShared) -> Self {
        Self { inner: Arc::new(inner) }
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
}
