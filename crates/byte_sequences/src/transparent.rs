// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use crate::{BlockSize, Memory, SequenceBuilder, std_alloc_block};

/// A memory provider that simply delegates 1:1 to the Rust global allocator.
///
/// This is meant for test scenarios where the minimal set of memory provider
/// functionality is desired, to establish maximally controlled conditions.
///
/// For general-purpose public use, the [`NeutralMemoryPool`][1] should be used instead,
/// as it is geared for actual efficiency - this here is just a simple passthrough implementation.
///
/// [1]: crate::NeutralMemoryPool
#[derive(Clone, Debug, Default)]
pub struct TransparentTestMemory {
    // We may add more fields later, so this is a placeholder to ensure we do not empty-type this.
    _placeholder: (),
}

impl TransparentTestMemory {
    /// Creates a new instance of the memory provider.
    #[must_use]
    pub const fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Reserves `len` bytes of mutable memory, returning an empty
    /// [`SequenceBuilder`] whose capacity is backed by the reserved memory.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`SequenceBuilder`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    #[expect(clippy::unused_self, reason = "for potential future functionality enrichment")]
    pub fn reserve(&self, len: usize) -> crate::SequenceBuilder {
        reserve(len)
    }
}

impl Memory for TransparentTestMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::SequenceBuilder {
        self.reserve(min_bytes)
    }
}

fn reserve(min_bytes: usize) -> crate::SequenceBuilder {
    let Some(min_bytes) = NonZero::new(min_bytes) else {
        return SequenceBuilder::default();
    };

    let block_count = min_bytes.get().div_ceil(BlockSize::MAX as usize);
    let mut bytes_remaining = min_bytes.get();

    let mut blocks = Vec::with_capacity(block_count);

    for _ in 0..block_count {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the usize never contains a value outside bounds of BlockSize - guarded by min()"
        )]
        let bytes_in_block = NonZero::new(bytes_remaining.min(BlockSize::MAX as usize) as BlockSize)
            .expect("ran out of bytes before calculated block count - the math must be wrong");

        bytes_remaining = bytes_remaining
            .checked_sub(bytes_in_block.get() as usize)
            .expect("negative bytes remaining - algorithm error");

        blocks.push(std_alloc_block::allocate(bytes_in_block));
    }

    SequenceBuilder::from_blocks(blocks)
}

#[cfg(test)]
mod tests {
    use bytes::BufMut;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::MemoryShared;

    assert_impl_all!(TransparentTestMemory: MemoryShared);

    #[test]
    fn smoke_test() {
        let memory = TransparentTestMemory::new();

        let mut sb = memory.reserve(0);
        assert_eq!(sb.len(), 0);
        assert_eq!(sb.capacity(), 0);

        sb = memory.reserve(1313);

        assert_eq!(sb.capacity(), 1313);

        sb.put_bytes(3, 1313);

        let sequence = sb.consume_all();

        assert_eq!(sequence.len(), 1313);
        assert_eq!(sequence.chunk().len(), 1313);
    }

    #[test]
    fn giant_allocation() {
        // This is a giant allocation that does not fit into one memory block.

        let memory = TransparentTestMemory::new();
        let sb = memory.reserve(5_000_000_000);

        assert_eq!(sb.capacity(), 5_000_000_000);

        // NB! We cannot simply check the first chunk length because there is no guarantee on which
        // order a SequenceBuilder consumes its blocks in - the first might not be u32::MAX here!
    }
}
