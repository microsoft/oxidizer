// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future;

use crate::mem::{FakeMemoryProvider, ProvideMemory, Sequence, SequenceBuilder};
use crate::{ReadStream, WriteStream};

/// A readable and writable stream that never completes any reads or writes.
/// Intended for simple tests and examples.
#[derive(Debug)]
pub struct PendingStream;

impl ProvideMemory for PendingStream {
    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        FakeMemoryProvider.reserve(min_bytes)
    }
}

impl ReadStream for PendingStream {
    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn read_at_most_into(
        &mut self,
        _len: usize,
        _into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)> {
        future::pending().await
    }

    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn read_more_into(
        &mut self,
        _into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)> {
        future::pending().await
    }

    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn read_any(&mut self) -> crate::Result<SequenceBuilder> {
        future::pending().await
    }
}

impl WriteStream for PendingStream {
    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn write(&mut self, _sequence: Sequence) -> crate::Result<()> {
        future::pending().await
    }
}