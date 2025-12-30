// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(any(test, feature = "test-util"))]

use std::iter;
use std::num::NonZero;
use std::sync::Arc;

use crate::{BlockSize, BytesBuf, Memory, std_alloc_block};

/// A memory provider that cuts the memory allocation into blocks of fixed size
/// and delegates to the Rust global allocator for allocating those blocks.
///
/// This is meant for test scenarios where a specific memory block size is important.
///
/// This memory provider is a simple implementation that does not perform any pooling
/// or performance optimization, so should not be used in real code.
#[derive(Clone, Debug)]
pub struct FixedBlockTestMemory {
    inner: Arc<FixedBlockTestMemoryInner>,
}

impl FixedBlockTestMemory {
    /// Creates a new instance of the memory provider.
    #[must_use]
    pub fn new(block_size: NonZero<BlockSize>) -> Self {
        Self {
            inner: Arc::new(FixedBlockTestMemoryInner::new(block_size)),
        }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.inner.reserve(min_bytes)
    }
}

impl Memory for FixedBlockTestMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

#[derive(Debug)]
struct FixedBlockTestMemoryInner {
    block_size: NonZero<BlockSize>,
}

impl FixedBlockTestMemoryInner {
    #[must_use]
    pub(crate) const fn new(block_size: NonZero<BlockSize>) -> Self {
        Self { block_size }
    }
}

impl FixedBlockTestMemoryInner {
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        let Some(min_bytes) = NonZero::new(min_bytes) else {
            return BytesBuf::default();
        };

        let blocks_required = min_bytes.get().div_ceil(self.block_size.get() as usize);

        let blocks = iter::repeat_with(|| std_alloc_block::allocate(self.block_size)).take(blocks_required);

        BytesBuf::from_blocks(blocks)
    }
}

#[cfg(test)]
mod tests {
    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::{BytesView, MemoryShared};

    assert_impl_all!(FixedBlockTestMemory: MemoryShared);

    #[test]
    fn byte_by_byte() {
        let memory = FixedBlockTestMemory::new(nz!(1));

        let sb = memory.reserve(0);
        assert_eq!(sb.len(), 0);
        assert_eq!(sb.capacity(), 0);

        let mut sequence = BytesView::copied_from_slice(b"Hello, world", &memory);
        assert_eq!(sequence, b"Hello, world");

        assert_eq!(sequence.first_slice().len(), 1);

        let mut chunks_encountered: usize = 0;

        sequence.consume_all_slices(|chunk| {
            chunks_encountered = chunks_encountered.saturating_add(1);
            assert_eq!(chunk.len(), 1);
        });

        assert_eq!(chunks_encountered, 12);
    }
}
