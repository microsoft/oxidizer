// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use crate::block::Block;
use crate::span_builder::SpanBuilder;
use crate::{MAX_BLOCK_SIZE, ProvideMemory, SequenceBuilder};

/// An implementation of [`ProvideMemory`] that simply allocates memory from the default
/// Rust memory allocator to create a memory pool.
#[derive(Debug, Clone)]
pub struct DefaultMemoryPool {
    block_size: NonZero<usize>,
}

impl DefaultMemoryPool {
    /// Creates a new `DefaultMemoryPool` with the specified block size.
    ///
    /// # Panics
    ///
    /// Panics if the block size exceeds the maximum allowed size of [`u32::MAX`].
    #[must_use]
    pub fn new(block_size: NonZero<usize>) -> Self {
        assert!(
            block_size.get() <= MAX_BLOCK_SIZE,
            "requested block size exceeds internal API compatibility thresholds"
        );

        Self { block_size }
    }
}

impl ProvideMemory for DefaultMemoryPool {
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        let block_count = min_bytes.div_ceil(self.block_size.get());
        let block_reference = (0..block_count).map(|_| Block::new(self.block_size));
        let span_builders = block_reference.map(SpanBuilder::new);

        SequenceBuilder::from_span_builders(span_builders)
    }
}

#[cfg(test)]
mod tests {
    use bytes::BufMut;
    use static_assertions::assert_impl_all;

    use super::*;

    #[test]
    fn smoke_test() {
        let pool = DefaultMemoryPool::new(NonZero::new(1234).unwrap());

        let min_length = 1000;

        let builder = pool.reserve(min_length);

        assert!(builder.remaining_mut() >= min_length);
        assert_eq!(builder.len(), 0);
    }

    #[test]
    fn zero_size() {
        let pool = DefaultMemoryPool::new(NonZero::new(1234).unwrap());

        let builder = pool.reserve(0);

        assert_eq!(builder.len(), 0);
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(DefaultMemoryPool: Send, Sync);
    }
}