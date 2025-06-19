// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Buf;

use crate::ReadStream;
use crate::mem::{FakeMemoryProvider, ProvideMemory, Sequence, SequenceBuilder};

/// A `ReadStream` implementation that reads data from fake I/O memory.
/// For test and example purposes only, not for real I/O.
#[derive(Debug)]
pub struct FakeReadStream {
    inner: Sequence,

    // For testing purposes, we may choose to limit the read size and
    // thereby force the caller to do multiple read operations.
    max_read_size: Option<usize>,
}

impl FakeReadStream {
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    #[must_use]
    pub const fn new(inner: Sequence) -> Self {
        Self {
            inner,
            max_read_size: None,
        }
    }

    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    #[must_use]
    pub const fn with_max_read_size(inner: Sequence, max_read_size: usize) -> Self {
        Self {
            inner,
            max_read_size: Some(max_read_size),
        }
    }
}

const PREFERRED_READ_SIZE: usize = 100;

impl ReadStream for FakeReadStream {
    async fn read_at_most_into(
        &mut self,
        len: usize,
        mut into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)> {
        let bytes_to_read = len
            .min(self.inner.remaining())
            .min(self.max_read_size.unwrap_or(usize::MAX));

        if bytes_to_read == 0 {
            return Ok((0, into));
        }

        let sequence_to_read = self.inner.slice(0..bytes_to_read);
        into.append(sequence_to_read);

        self.inner.advance(bytes_to_read);

        Ok((bytes_to_read, into))
    }

    async fn read_more_into(
        &mut self,
        into: SequenceBuilder,
    ) -> crate::Result<(usize, SequenceBuilder)> {
        self.read_at_most_into(PREFERRED_READ_SIZE, into).await
    }

    async fn read_any(&mut self) -> crate::Result<SequenceBuilder> {
        Ok(self
            .read_at_most_into(PREFERRED_READ_SIZE, SequenceBuilder::new())
            .await?
            .1)
    }
}

impl ProvideMemory for FakeReadStream {
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        FakeMemoryProvider.reserve(min_bytes)
    }
}

#[cfg(test)]
mod tests {
    use bytes::BufMut;

    use super::*;
    use crate::ReadStreamExt;
    use crate::testing::async_test;

    #[test]
    fn smoke_test() {
        async_test! {
            let mut sb = FakeMemoryProvider.reserve(100);
            sb.put_u8(1);
            sb.put_u8(2);
            sb.put_u8(3);

            let sequence = sb.consume_all();

            let mut read_stream = FakeReadStream::new(sequence);

            let mut payload = read_stream.read_exactly(3).await.unwrap();
            assert_eq!(payload.len(), 3);

            assert_eq!(payload.get_u8(), 1);
            assert_eq!(payload.get_u8(), 2);
            assert_eq!(payload.get_u8(), 3);

            let final_read = read_stream.read_any().await.unwrap();
            assert_eq!(final_read.len(), 0);
        }
    }

    #[test]
    fn with_max_read_size() {
        async_test! {
            let test_data = FakeMemoryProvider::copy_from_static(b"Hello, world!");
            let mut read_stream = FakeReadStream::with_max_read_size(test_data, 2);

            // He
            let mut payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_u8(), b'H');
            assert_eq!(payload.get_u8(), b'e');

            // ll
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_u8(), b'l');
            assert_eq!(payload.get_u8(), b'l');

            // o,
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_u8(), b'o');
            assert_eq!(payload.get_u8(), b',');

            //  w
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_u8(), b' ');
            assert_eq!(payload.get_u8(), b'w');

            // or
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_u8(), b'o');
            assert_eq!(payload.get_u8(), b'r');

            // ld
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_u8(), b'l');
            assert_eq!(payload.get_u8(), b'd');

            // !
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 1);
            assert_eq!(payload.get_u8(), b'!');
        }
    }

    #[test]
    fn read_more() {
        async_test! {
            let test_data = FakeMemoryProvider::copy_from_static(b"Hello, world!");
            let mut read_stream = FakeReadStream::with_max_read_size(test_data.clone(), 2);

            let mut payload_buffer = read_stream.reserve(100);

            loop {
                let (bytes_read, new_payload_buffer) = read_stream
                    .read_more_into(payload_buffer).await.unwrap();

                payload_buffer = new_payload_buffer;

                if bytes_read == 0 {
                    break;
                }
            }

            assert_eq!(payload_buffer.len(), test_data.len());

            let mut payload = payload_buffer.consume_all();
            assert_eq!(payload.len(), test_data.len());
            assert_eq!(payload.get_u8(), b'H');
            assert_eq!(payload.get_u8(), b'e');
            assert_eq!(payload.get_u8(), b'l');
            assert_eq!(payload.get_u8(), b'l');
            assert_eq!(payload.get_u8(), b'o');
            assert_eq!(payload.get_u8(), b',');
            assert_eq!(payload.get_u8(), b' ');
            assert_eq!(payload.get_u8(), b'w');
            assert_eq!(payload.get_u8(), b'o');
            assert_eq!(payload.get_u8(), b'r');
            assert_eq!(payload.get_u8(), b'l');
            assert_eq!(payload.get_u8(), b'd');
            assert_eq!(payload.get_u8(), b'!');
            assert_eq!(payload.len(), 0);
        }
    }
}