// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use crate::BytesBuf;
use crate::mem::testing::std_alloc_block;
use crate::mem::{BlockSize, Memory};

/// A memory provider that delegates 1:1 to the Rust global allocator.
///
/// This is meant for test scenarios where the minimal set of memory provider
/// functionality is desired, to establish maximally controlled conditions.
///
/// # Performance
///
/// For general-purpose public use, the [`GlobalPool`][1] should be used instead,
/// as it is geared for actual efficiency - this here is just a simple passthrough implementation.
///
/// [1]: crate::mem::GlobalPool
#[derive(Clone, Debug, Default)]
pub struct TransparentMemory {
    // We may add more fields later, so this is a placeholder to ensure we do not empty-type this.
    _placeholder: (),
}

impl TransparentMemory {
    /// Creates a new instance of the memory provider.
    #[must_use]
    pub const fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Reserves exactly `min_bytes` bytes of memory capacity.
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
    #[expect(clippy::unused_self, reason = "for potential future functionality enrichment")]
    pub fn reserve(&self, len: usize) -> crate::BytesBuf {
        reserve(len)
    }
}

impl Memory for TransparentMemory {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

fn reserve(min_bytes: usize) -> crate::BytesBuf {
    let Some(min_bytes) = NonZero::new(min_bytes) else {
        return BytesBuf::default();
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

    BytesBuf::from_blocks(blocks)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::mem::MemoryShared;

    assert_impl_all!(TransparentMemory: MemoryShared);

    #[test]
    fn smoke_test() {
        let memory = TransparentMemory::new();

        let mut buf = memory.reserve(0);
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 0);

        buf = memory.reserve(1313);

        assert_eq!(buf.capacity(), 1313);

        buf.put_byte_repeated(3, 1313);

        let data = buf.consume_all();

        assert_eq!(data.len(), 1313);
        assert_eq!(data.first_slice().len(), 1313);
    }

    #[test]
    fn giant_allocation() {
        // This test requires at least 5 GB of memory to run. The publishing pipeline runs on a system
        // where this may not be available, so we skip this test in that environment.
        #[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
        if crate::testing::system_memory() < 10_000_000_000 {
            eprintln!("Skipping giant allocation test due to insufficient memory.");
            return;
        }

        // This is a giant allocation that does not fit into one memory block.

        let memory = TransparentMemory::new();
        let buf = memory.reserve(5_000_000_000);

        assert_eq!(buf.capacity(), 5_000_000_000);

        // NB! We cannot simply check the first chunk length because there is no guarantee on which
        // order a BytesBuf consumes its blocks in - the first might not be u32::MAX here!
    }
}
