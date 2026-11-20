// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "futures-stream")]
use std::pin::Pin;

use bytesbuf::{BytesBuf, BytesView};

use crate::Read;
#[cfg(feature = "futures-stream")]
use crate::ReadAsFuturesStream;

/// Universal convenience methods built on top of [`Read`].
///
/// This trait is implemented for all types that implement [`Read`].
#[trait_variant::make(Send)]
pub trait ReadExt: Read {
    /// Reads at most `len` bytes into a new buffer.
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
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::Null;
    /// use bytesbuf_io::ReadExt;
    ///
    /// # fn get_source() -> Null { Null::new() }
    /// let mut source = get_source();
    ///
    /// let data = source.read_at_most(123).await.unwrap();
    ///
    /// println!("read {} bytes of data", data.len());
    /// # }));
    /// # }
    /// # }
    /// ```
    async fn read_at_most(&mut self, len: usize) -> crate::Result<BytesView>;

    /// Reads exactly `len` bytes into a new buffer.
    ///
    /// The call will complete only when enough data has been read (or an error occurs).
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::FakeRead;
    /// # use bytesbuf::BytesView;
    /// # use bytesbuf::mem::GlobalPool;
    /// use bytesbuf_io::ReadExt;
    ///
    /// # fn get_source() -> FakeRead { FakeRead::new(BytesView::copied_from_slice(b"0123456789012345667890", &GlobalPool::new())) }
    /// let mut source = get_source();
    ///
    /// let data = source.read_exactly(10).await.unwrap();
    /// assert_eq!(data.len(), 10);
    /// # }));
    /// # }
    /// # }
    /// ```
    async fn read_exactly(&mut self, len: usize) -> crate::Result<BytesView>;

    /// Conditionally reads at most `len` bytes into a new buffer.
    ///
    /// The provided inspector function is used to inspect the data as it is read and to decide
    /// whether to continue reading or stop/fail the operation.
    async fn read_at_most_while<F>(&mut self, len: usize, inspect_fn: F) -> crate::Result<(BytesView, BytesBuf)>
    where
        F: FnMut(BytesView) -> ReadInspectDecision + Send;

    /// Conditionally reads at most `len` bytes into the provided buffer.
    ///
    /// The provided inspector function is used to inspect the data as it is read and to decide
    /// whether to continue reading or stop/fail the operation.
    ///
    /// # Panics
    ///
    /// Panics if the provided `BytesBuf` already contains `len` or more bytes, as this is
    /// likely to be a programming error that may lead to an infinite loop if not guarded against.
    async fn read_at_most_into_while<F>(&mut self, len: usize, into: BytesBuf, inspect_fn: F) -> crate::Result<(BytesView, BytesBuf)>
    where
        F: FnMut(BytesView) -> ReadInspectDecision + Send;

    /// Transforms the `Read` into a `futures::Stream`.
    ///
    /// Each item yielded by the stream corresponds to a sequence of one or more bytes read from this source.
    #[cfg(feature = "futures-stream")]
    fn into_futures_stream(self) -> Pin<Box<ReadAsFuturesStream<Self>>>
    where
        Self: Sized;
}

impl<T> ReadExt for T
where
    T: Read,
{
    async fn read_at_most(&mut self, len: usize) -> crate::Result<BytesView> {
        self.read_at_most_into(len, self.reserve(len))
            .await
            .map(|(_bytes_read, mut buffer)| buffer.consume_all())
            .map_err(crate::Error::caused_by)
    }

    async fn read_exactly(&mut self, len: usize) -> crate::Result<BytesView> {
        let mut buffer = self.reserve(len);

        while buffer.len() < len {
            let remaining = len.checked_sub(buffer.len()).expect("we validated above that this cannot overflow");

            let (bytes_read, new_buffer) = self.read_at_most_into(remaining, buffer).await.map_err(crate::Error::caused_by)?;
            buffer = new_buffer;

            if bytes_read == 0 {
                return Err(crate::Error::caused_by(
                    "source was closed before exact number of bytes could be read",
                ));
            }
        }

        assert_eq!(buffer.len(), len);

        Ok(buffer.consume_all())
    }

    async fn read_at_most_while<F>(&mut self, len: usize, inspect_fn: F) -> crate::Result<(BytesView, BytesBuf)>
    where
        F: FnMut(BytesView) -> ReadInspectDecision + Send,
    {
        let buffer = self.reserve(len);
        self.read_at_most_into_while(len, buffer, inspect_fn).await
    }

    async fn read_at_most_into_while<F>(
        &mut self,
        len: usize,
        mut into: BytesBuf,
        mut inspect_fn: F,
    ) -> crate::Result<(BytesView, BytesBuf)>
    where
        F: FnMut(BytesView) -> ReadInspectDecision + Send,
    {
        assert!(into.len() < len, "bytes already present must be smaller than len");

        while into.len() < len {
            let remaining = len.checked_sub(into.len()).expect("we validated above that this cannot overflow");

            let (bytes_read, new_into) = self.read_at_most_into(remaining, into).await.map_err(crate::Error::caused_by)?;
            into = new_into;

            if bytes_read == 0 {
                return Err(crate::Error::caused_by("source was closed before conditional read was completed"));
            }

            match inspect_fn(into.peek()) {
                ReadInspectDecision::ContinueRead => {}
                ReadInspectDecision::Complete(bytes) => {
                    return Ok((into.consume(bytes), into));
                }
                ReadInspectDecision::Failed(err) => {
                    return Err(crate::Error::caused_by(err));
                }
            }
        }

        // If we got here, we ran out of our byte budget before we were signaled to end.
        Err(crate::Error::caused_by("byte budget exceeded while performing conditional read"))
    }

    #[cfg(feature = "futures-stream")]
    fn into_futures_stream(self) -> Pin<Box<ReadAsFuturesStream<Self>>>
    where
        Self: Sized,
    {
        ReadAsFuturesStream::new(self)
    }
}

/// A flow control decision made during conditional reads.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "not designed for extensibility; we accept that any change will be breaking"
)]
pub enum ReadInspectDecision {
    /// Continues reading.
    ///
    /// If the byte budget has been exhausted for "read at most `N` bytes" reads (i.e. there are already `N` bytes buffered),
    /// the read terminates with an error.
    ContinueRead,

    /// Completes reading from the stream, consuming the indicated number of bytes.
    ///
    /// Any remaining bytes are kept in a `BytesBuf` returned alongside the read data,
    /// allowing the buffer to be efficiently reused for a follow-up read operation.
    Complete(usize),

    /// Stops reading and signals the provided error as the result.
    ///
    /// The provided error will be wrapped in a [`bytesbuf_io::Error`][crate::Error].
    Failed(Box<dyn std::error::Error + Send + Sync>),
}
