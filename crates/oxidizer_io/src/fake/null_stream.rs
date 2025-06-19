// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::mem::{FakeMemoryProvider, ProvideMemory, Sequence, SequenceBuilder};
use crate::{ReadStream, WriteStream};

/// A readable and writable stream that does nothing - any data written to it is discarded
/// and it never returns any data when read from. Intended for simple tests and examples.
#[derive(Debug, Default)]
pub struct NullStream;

impl NullStream {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ProvideMemory for NullStream {
    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        FakeMemoryProvider.reserve(min_bytes)
    }
}

impl ReadStream for NullStream {
    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn read_at_most_into(
        &mut self,
        _len: usize,
        into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)> {
        Ok((0, into))
    }

    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn read_more_into(
        &mut self,
        into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)> {
        Ok((0, into))
    }

    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn read_any(&mut self) -> crate::Result<SequenceBuilder> {
        Ok(SequenceBuilder::default())
    }
}

impl WriteStream for NullStream {
    #[cfg_attr(test, mutants::skip)] // Test/example code, do not waste time mutating.
    async fn write(&mut self, _sequence: Sequence) -> crate::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::BufMut;
    use oxidizer_testing::execute_or_terminate_process;

    use super::*;

    #[test]
    fn smoke_test() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = NullStream::new();

                let buffer = s.reserve(1000);
                assert!(buffer.remaining_mut() >= 1000);

                let (bytes_read, buffer) = s.read_at_most_into(100, buffer).await.unwrap();
                assert_eq!(bytes_read, 0);
                assert_eq!(buffer.len(), 0);

                let (bytes_read, buffer) = s.read_more_into(buffer).await.unwrap();
                assert_eq!(bytes_read, 0);
                assert_eq!(buffer.len(), 0);

                let mut buffer = s.read_any().await.unwrap();
                assert_eq!(buffer.len(), 0);

                s.write(buffer.consume_all()).await.unwrap();
            });
        });
    }
}