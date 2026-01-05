// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::convert::Infallible;
use std::num::NonZero;

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};

use crate::Read;

// Arbitrary number to fulfill the API contract that the stream decides its own optimal read size.
// As this is a fake stream, we could also move this value to the builder as a configurable option,
// but for now, we keep it simple because no scenario where that was relevant has appeared yet.
const PREFERRED_READ_SIZE: usize = 100;

/// A [`Read`] that reads from a [`BytesView`].
///
/// This is for test and example purposes only and is not optimized for performance.
#[derive(Debug)]
pub struct FakeRead {
    contents: BytesView,

    // For testing purposes, we may choose to limit the read size and
    // thereby force the caller to do multiple read operations.
    max_read_size: Option<NonZero<usize>>,

    memory: OpaqueMemory,
}

impl FakeRead {
    /// Starts building a new `FakeRead`.
    #[must_use]
    pub fn builder() -> FakeReadBuilder {
        FakeReadBuilder {
            contents: None,
            max_read_size: None,
            memory: OpaqueMemory::new(TransparentMemory::new()),
        }
    }

    /// Creates a new `FakeRead` with the given contents and the default configuration.
    #[must_use]
    pub fn new(contents: BytesView) -> Self {
        Self::builder().contents(contents).build()
    }

    /// Reads at most `len` bytes into the provided buffer.
    ///
    /// It is not necessary for `into` to be empty - the buffer may already have some
    /// bytes of data in it (e.g. from a previous read).
    ///
    /// The buffer will be extended with additional memory capacity
    /// if it does not have enough remaining capacity to fit `len` additional bytes.
    ///
    /// Returns a tuple of the number of bytes read and the updated buffer.
    ///
    /// The returned [`BytesBuf`] will have 0 or more bytes of data appended to it on success,
    /// with 0 appended bytes indicating that no more bytes can be read from this source. Any
    /// data that was already in the buffer will remain untouched.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[cfg_attr(test, mutants::skip)] // Mutations easily lead to infinite loops, not worth the effort.
    #[expect(clippy::unused_async, reason = "API compatibility between trait and inherent fn")]
    pub async fn read_at_most_into(&mut self, len: usize, mut into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        let bytes_to_read = len
            .min(self.contents.len())
            .min(self.max_read_size.map_or(usize::MAX, NonZero::get));

        if bytes_to_read == 0 {
            return Ok((0, into));
        }

        let data_to_read = self.contents.range(0..bytes_to_read);
        into.put_bytes(data_to_read);

        self.contents.advance(bytes_to_read);

        Ok((bytes_to_read, into))
    }

    // Reads an unspecified number of bytes into the provided buffer.
    ///
    /// The implementation will decide how many bytes to read based on its internal understanding of
    /// what is optimal for sustained throughput at high efficiency. This may be a fixed size,
    /// or it may be a variable size based on the current state of the source.
    ///
    /// It is not necessary for `into` to be empty - the buffer may already have some
    /// bytes of data in it (e.g. from a previous read).
    ///
    /// The buffer will be extended with additional memory capacity
    /// if it does not have enough remaining capacity to fit `len` additional bytes.
    ///
    /// Returns a tuple of the number of bytes read and the updated buffer.
    ///
    /// The returned [`BytesBuf`] will have 0 or more bytes of data appended to it on success,
    /// with 0 appended bytes indicating that no more bytes can be read from this source. Any
    /// data that was already in the buffer will remain untouched.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[cfg_attr(test, mutants::skip)] // Mutations easily lead to infinite loops, not worth the effort.
    pub async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        let previous_len = into.len();

        self.read_at_most_into(PREFERRED_READ_SIZE, into)
            .await
            .inspect(|result| debug_assert_eq!(previous_len + result.0, result.1.len()))
    }

    /// Reads an unspecified number of bytes as a new buffer.
    ///
    /// The implementation will decide how many bytes to read based on its internal understanding of
    /// what is optimal for sustained throughput at high efficiency. This may be a fixed size,
    /// or it may be a variable size based on the current state of the source.
    ///
    /// The returned [`BytesBuf`] will have 0 or more bytes of data appended to it on success,
    /// with 0 appended bytes indicating that no more bytes can be read from this source.
    ///
    /// # Security
    ///
    /// **This method is insecure if the side producing the bytes is not trusted**. An attacker
    /// may trickle data byte-by-byte, consuming a large amount of I/O resources.
    ///
    /// Robust code working with untrusted sources should take precautions such as only processing
    /// read data when either a time or length threshold is reached and reusing buffers that
    /// have remaining capacity, appending additional data to existing buffers using
    /// [`read_more_into()`][crate::Read::read_more_into] instead of reserving new buffers
    /// for each read operation.
    ///
    /// # Errors
    ///
    /// This call never fails.
    pub async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        Ok(self.read_at_most_into(PREFERRED_READ_SIZE, BytesBuf::new()).await?.1)
    }

    /// Returns the memory provider that was configured in the builder.
    #[must_use]
    pub fn memory(&self) -> impl MemoryShared {
        self.memory.clone()
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

impl Read for FakeRead {
    type Error = Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn read_at_most_into(&mut self, len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        self.read_at_most_into(len, into).await
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        self.read_more_into(into).await
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        self.read_any().await
    }
}

impl Memory for FakeRead {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

impl HasMemory for FakeRead {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn memory(&self) -> impl MemoryShared {
        self.memory()
    }
}

/// Creates an instance of [`FakeRead`].
///
/// Access through [`FakeRead::builder()`][FakeRead::builder].
#[derive(Debug)]
pub struct FakeReadBuilder {
    contents: Option<BytesView>,
    max_read_size: Option<NonZero<usize>>,
    memory: OpaqueMemory,
}

impl FakeReadBuilder {
    /// The data to return from read operations. Mandatory.
    #[must_use]
    pub fn contents(mut self, contents: BytesView) -> Self {
        self.contents = Some(contents);
        self
    }

    /// Restricts the result of a single read operation to at most `max_read_size` bytes.
    ///
    /// Optional. Defaults to no limit.
    #[must_use]
    pub fn max_read_size(mut self, max_read_size: NonZero<usize>) -> Self {
        self.max_read_size = Some(max_read_size);
        self
    }

    /// The memory provider to use in memory-related stream operations.
    ///
    /// Optional. Defaults to using the Rust global allocator.
    #[must_use]
    pub fn memory(mut self, memory: OpaqueMemory) -> Self {
        self.memory = memory;
        self
    }

    /// Builds the `FakeRead` with the provided configuration.
    ///
    /// # Panics
    ///
    /// Panics if the contents of the stream have not been set.
    #[must_use]
    pub fn build(self) -> FakeRead {
        assert!(self.contents.is_some(), "{} requires a sequence to be set", type_name::<Self>());

        FakeRead {
            contents: self.contents.expect("guarded by assertion above"),
            max_read_size: self.max_read_size,
            memory: self.memory,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytesbuf::mem::GlobalPool;
    use new_zealand::nz;
    use testing_aids::async_test;

    use super::*;
    use crate::ReadExt;

    #[test]
    fn smoke_test() {
        async_test(async || {
            let memory = GlobalPool::new();
            let mut buf = memory.reserve(100);
            buf.put_byte(1);
            buf.put_byte(2);
            buf.put_byte(3);

            let contents = buf.consume_all();

            let mut read_stream = FakeRead::new(contents);

            let stream_memory = read_stream.reserve(1234);
            assert!(stream_memory.capacity() >= 1234);

            let mut payload = read_stream.read_exactly(3).await.unwrap();
            assert_eq!(payload.len(), 3);

            assert_eq!(payload.get_byte(), 1);
            assert_eq!(payload.get_byte(), 2);
            assert_eq!(payload.get_byte(), 3);

            let final_read = read_stream.read_any().await.unwrap();
            assert_eq!(final_read.len(), 0);
        });
    }

    #[test]
    fn with_max_read_size() {
        async_test(async || {
            let memory = GlobalPool::new();
            let test_data = BytesView::copied_from_slice(b"Hello, world!", &memory);
            let mut read_stream = FakeRead::builder().contents(test_data).max_read_size(nz!(2)).build();

            // He
            let mut payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'H');
            assert_eq!(payload.get_byte(), b'e');

            // ll
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'l');

            // o,
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b',');

            //  w
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b' ');
            assert_eq!(payload.get_byte(), b'w');

            // or
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b'r');

            // ld
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'd');

            // !
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 1);
            assert_eq!(payload.get_byte(), b'!');
        });
    }

    #[test]
    fn read_more() {
        async_test(async || {
            let memory = GlobalPool::new();
            let test_data = BytesView::copied_from_slice(b"Hello, world!", &memory);
            let mut read_stream = FakeRead::builder().contents(test_data.clone()).max_read_size(nz!(2)).build();

            let mut payload_buffer = read_stream.reserve(100);

            loop {
                let previous_len = payload_buffer.len();

                let (bytes_read, new_payload_buffer) = read_stream.read_more_into(payload_buffer).await.unwrap();

                payload_buffer = new_payload_buffer;

                // Sanity check.
                assert_eq!(previous_len + bytes_read, payload_buffer.len());

                if bytes_read == 0 {
                    break;
                }
            }

            assert_eq!(payload_buffer.len(), test_data.len());

            let mut payload = payload_buffer.consume_all();
            assert_eq!(payload.len(), test_data.len());
            assert_eq!(payload.get_byte(), b'H');
            assert_eq!(payload.get_byte(), b'e');
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b',');
            assert_eq!(payload.get_byte(), b' ');
            assert_eq!(payload.get_byte(), b'w');
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b'r');
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'd');
            assert_eq!(payload.get_byte(), b'!');
            assert_eq!(payload.len(), 0);
        });
    }
}
