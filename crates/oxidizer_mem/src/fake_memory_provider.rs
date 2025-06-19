// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use bytes::BufMut;

use crate::{Block, ProvideMemory, Sequence, SequenceBuilder, SpanBuilder};

/// Provides fake I/O memory for test and example purposes.
///
/// The fake memory is functional enough for tests, but the implementation is not compatible
/// with the real I/O memory management logic, so this cannot be used with real I/O code.
#[derive(Debug)]
pub struct FakeMemoryProvider;

impl FakeMemoryProvider {
    /// Copies a static byte slice into a `Sequence`, for simple input of static test data.
    #[must_use]
    pub fn copy_from_static(bytes: &'static [u8]) -> Sequence {
        let mut buffer = Self::reserve(&Self, bytes.len());
        buffer.put_slice(bytes);
        buffer.consume_all()
    }
}

impl ProvideMemory for FakeMemoryProvider {
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        // We always provide a little bit of extra memory, to avoid anyone taking a dependency
        // on the exact size of the buffer. A memory provider can always return more capacity!
        const EXTRA_MEMORY: usize = 7;

        let min_bytes = NonZero::new(min_bytes.saturating_add(EXTRA_MEMORY))
            .expect("impossible to reach zero if we always add some nonzero capacitry");

        let block = Block::new(min_bytes);
        let span_builder = SpanBuilder::new(block);
        SequenceBuilder::from_span_builders([span_builder])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_does_what_it_is_supposed_to() {
        let buffer = FakeMemoryProvider.reserve(1000);
        assert!(buffer.remaining_mut() >= 1000);

        // Every length is valid, we just want to verify no panic occurs.
        FakeMemoryProvider.reserve(0);
    }

    #[test]
    fn copy_from_static() {
        let bytes = b"Hello, world!";
        let sequence = FakeMemoryProvider::copy_from_static(bytes);
        assert_eq!(sequence.len(), bytes.len());
    }
}