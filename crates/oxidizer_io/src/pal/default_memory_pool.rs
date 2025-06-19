// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use crate::mem::{ProvideMemory, SequenceBuilder};
use crate::pal::MemoryPool;

/// An implementation of [`MemoryPool`] that simply allocates memory from the default
/// Rust memory allocator to create a memory pool.
#[derive(Debug)]
pub struct DefaultMemoryPool {
    inner: oxidizer_mem::DefaultMemoryPool,
    block_size: NonZero<usize>,
}

impl DefaultMemoryPool {
    /// Creates a new `DefaultMemoryPool` with the specified block size.
    pub(crate) fn new(block_size: NonZero<usize>) -> Self {
        Self {
            block_size,
            inner: oxidizer_mem::DefaultMemoryPool::new(block_size),
        }
    }
}

impl MemoryPool for DefaultMemoryPool {
    fn rent(&self, count_bytes: usize, _preferred_block_size: NonZero<usize>) -> SequenceBuilder {
        self.inner.reserve(count_bytes)
    }
}

impl ProvideMemory for DefaultMemoryPool {
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        self.rent(min_bytes, self.block_size)
    }
}

#[cfg(test)]
mod tests {
    use bytes::BufMut;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::nz;

    #[test]
    fn smoke_test() {
        let pool = DefaultMemoryPool::new(nz!(1234));

        let min_length = 1000;
        let preferred_block_size = nz!(100);

        let builder = pool.rent(min_length, preferred_block_size);

        assert!(builder.remaining_mut() >= min_length);
        assert_eq!(builder.len(), 0);

        let builder = pool.reserve(min_length);

        assert!(builder.remaining_mut() >= min_length);
        assert_eq!(builder.len(), 0);
    }

    #[test]
    fn zero_size() {
        let pool = DefaultMemoryPool::new(nz!(1234));

        let preferred_block_size = nz!(100);

        let builder = pool.rent(0, preferred_block_size);

        assert_eq!(builder.len(), 0);
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(DefaultMemoryPool: Send, Sync);
    }
}