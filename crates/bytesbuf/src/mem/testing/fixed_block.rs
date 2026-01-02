// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::iter;
use std::num::NonZero;
use std::sync::Arc;

use crate::BytesBuf;
use crate::mem::testing::std_alloc_block;
use crate::mem::{BlockSize, Memory};

/// A memory provider that uses fixed-size memory blocks.
///
/// Every memory capacity reservation is cut into into blocks of fixed size
/// and delegated to the Rust global allocator, which provides the actual memory capacity.
///
/// This provider is meant for test scenarios where a specific memory block size is important,
/// such as when testing edge cases of multi-block byte sequence handling. You can go down
/// as low as 1 byte per block to simulate extreme memory fragmentation. All user code is
/// expected to correctly operate with memory blocks of any size, including single-byte blocks.
///
/// # Performance
///
/// This memory provider is a simple implementation that does not perform any pooling
/// or performance optimization, so should not be used in real code.
///
/// # Examples
///
/// Reserving memory returns capacity rounded up to the block size:
///
/// ```
/// use std::num::NonZero;
///
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::Memory;
/// use bytesbuf::mem::testing::FixedBlockMemory;
///
/// // Create a memory provider with 16-byte blocks.
/// let memory = FixedBlockMemory::new(NonZero::new(16).unwrap());
///
/// // Request 10 bytes - capacity is rounded up to 16 (one full block).
/// let buf = memory.reserve(10);
/// assert_eq!(buf.capacity(), 16);
///
/// // Request 20 bytes - capacity is rounded up to 32 (two full blocks).
/// let buf = memory.reserve(20);
/// assert_eq!(buf.capacity(), 32);
///
/// // Data spanning multiple blocks is split across slices.
/// let mut data = BytesView::copied_from_slice(b"Hello, world! This is a test!", &memory);
/// assert_eq!(data.len(), 29);
/// assert_eq!(data.first_slice().len(), 16); // First block is full.
/// data.advance(16);
/// assert_eq!(data.first_slice().len(), 13); // Remaining 13 bytes in second block.
/// ```
#[derive(Clone, Debug)]
pub struct FixedBlockMemory {
    inner: Arc<FixedBlockMemoryInner>,
}

impl FixedBlockMemory {
    /// Creates a new instance of the memory provider.
    #[must_use]
    pub fn new(block_size: NonZero<BlockSize>) -> Self {
        Self {
            inner: Arc::new(FixedBlockMemoryInner::new(block_size)),
        }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// The requested amount `min_bytes` is rounded up to the nearest multiple of the fixed block size.
    ///
    /// Returns a [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.inner.reserve(min_bytes)
    }
}

impl Memory for FixedBlockMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

#[derive(Debug)]
struct FixedBlockMemoryInner {
    block_size: NonZero<BlockSize>,
}

impl FixedBlockMemoryInner {
    #[must_use]
    pub(crate) const fn new(block_size: NonZero<BlockSize>) -> Self {
        Self { block_size }
    }
}

impl FixedBlockMemoryInner {
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        let Some(min_bytes) = NonZero::new(min_bytes) else {
            return BytesBuf::default();
        };

        let blocks_required = min_bytes.get().div_ceil(self.block_size.get() as usize);

        let blocks = iter::repeat_with(|| std_alloc_block::allocate(self.block_size)).take(blocks_required);

        BytesBuf::from_blocks(blocks)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::BytesView;
    use crate::mem::MemoryShared;

    assert_impl_all!(FixedBlockMemory: MemoryShared);

    #[test]
    fn byte_by_byte() {
        let memory = FixedBlockMemory::new(nz!(1));

        let buf = memory.reserve(0);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);

        let mut data = BytesView::copied_from_slice(b"Hello, world", &memory);
        assert_eq!(data, b"Hello, world");

        assert_eq!(data.first_slice().len(), 1);

        let mut chunks_encountered: usize = 0;

        data.consume_all_slices(|chunk| {
            chunks_encountered = chunks_encountered.saturating_add(1);
            assert_eq!(chunk.len(), 1);
        });

        assert_eq!(chunks_encountered, 12);
    }
}
