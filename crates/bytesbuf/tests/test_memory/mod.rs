// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use bytesbuf::BytesBuf;
use bytesbuf::mem::{BlockSize, Memory};

// Reuse the allocation fixture through the public Block and BlockRef APIs.
#[path = "../../src/mem/testing/std_alloc_block.rs"]
mod std_alloc_block;

/// Controls block sizes without enabling the library's `std` feature.
#[derive(Debug, Default)]
pub(crate) struct TestMemory {
    block_size: Option<NonZero<BlockSize>>,
}

impl TestMemory {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) const fn with_block_size(block_size: NonZero<BlockSize>) -> Self {
        Self {
            block_size: Some(block_size),
        }
    }

    pub(crate) fn reserve(&self, min_bytes: usize) -> BytesBuf {
        let mut remaining = min_bytes;
        let blocks = std::iter::from_fn(|| {
            NonZero::new(remaining)?;
            let len = self
                .block_size
                .map_or_else(|| remaining.min(BlockSize::MAX as usize), |size| size.get() as usize);
            remaining = remaining.saturating_sub(len);
            Some(std_alloc_block::allocate(NonZero::new(BlockSize::try_from(len).unwrap()).unwrap()))
        });
        BytesBuf::from_blocks(blocks)
    }
}

impl Memory for TestMemory {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}
