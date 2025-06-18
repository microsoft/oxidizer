// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{self, ErrorKind};
use std::pin::Pin;

use crate::mem::{Sequence, SequenceBuilder, SequenceBuilderInspector};
use crate::{ReadStream, ReadStreamAsFuturesStream};

/// Convenience methods built on top of [`ReadStream`] to make it easier to read from a stream.
///
/// This trait is implemented for all types that implement [`ReadStream`].
#[trait_variant::make(Send)]
pub trait ReadStreamExt {
    /// Reads at most `len` bytes from the stream into a new sequence builder.
    ///
    /// # Security
    ///
    /// This method is not secure if the the other side of the stream is not trusted. An attacker
    /// may trickle data byte-by-byte, consuming a large amount of I/O resources.
    ///
    /// Robust code working with untrusted streams should take precautions such as only processing
    /// read data when either a time or length threshold is reached and reusing byte sequences that
    /// have remaining capacity, meanwhile appending to existing memory using
    /// [`read_more_into()`][crate::ReadStream::read_more_into] instead of reserving new memory
    /// for each read operation.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::NullStream;
    /// use oxidizer_io::ReadStreamExt;
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// let sequence = stream.read_at_most(123).await.unwrap();
    ///
    /// println!("read {} bytes of data", sequence.len());
    /// # }));
    /// ```
    async fn read_at_most(&mut self, len: usize) -> crate::Result<Sequence>;

    /// Reads exactly `len` bytes from the stream into a new sequence builder, waiting until
    /// enough data has been read.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::FakeReadStream;
    /// # use oxidizer_io::mem::FakeMemoryProvider;
    /// use oxidizer_io::ReadStreamExt;
    ///
    /// # fn get_stream() -> FakeReadStream { FakeReadStream::new(FakeMemoryProvider::copy_from_static(b"0123456789012345667890")) }
    /// let mut stream = get_stream();
    ///
    /// let sequence = stream.read_exactly(10).await.unwrap();
    /// assert_eq!(sequence.len(), 10);
    /// # }));
    /// ```
    async fn read_exactly(&mut self, len: usize) -> crate::Result<Sequence>;

    /// Reads at most `len` bytes from the stream into a new sequence builder, using the provided
    /// inspector function to inspect the data as it is read and to decide whether to continue
    /// reading or stop with success/error.
    async fn read_at_most_while<F>(
        &mut self,
        len: usize,
        inspect_fn: F,
    ) -> crate::Result<(Sequence, SequenceBuilder)>
    where
        F: FnMut(SequenceBuilderInspector) -> ReadInspectDecision + Send;

    /// Reads at most `len` bytes from the stream into the provided sequence builder, using the
    /// provided inspector function to inspect the data as it is read and to decide whether to
    /// continue reading or stop with success/error.
    async fn read_at_most_into_while<F>(
        &mut self,
        len: usize,
        into: SequenceBuilder,
        inspect_fn: F,
    ) -> crate::Result<(Sequence, SequenceBuilder)>
    where
        F: FnMut(SequenceBuilderInspector) -> ReadInspectDecision + Send;

    /// Transforms the `ReadStream` into a [`futures::Stream`].
    fn into_futures_stream(self) -> Pin<Box<ReadStreamAsFuturesStream<Self>>>
    where
        Self: ReadStream + Sized;
}

impl<T> ReadStreamExt for T
where
    T: ReadStream,
{
    async fn read_at_most(&mut self, len: usize) -> crate::Result<Sequence> {
        self.read_at_most_into(len, self.reserve(len))
            .await
            .map(|(_bytes_read, mut buffer)| buffer.consume_all())
    }

    async fn read_exactly(&mut self, len: usize) -> crate::Result<Sequence> {
        let mut buffer = self.reserve(len);

        while buffer.len() < len {
            let remaining = len
                .checked_sub(buffer.len())
                .expect("we validated above that this cannot overflow");

            let (bytes_read, new_buffer) = self.read_at_most_into(remaining, buffer).await?;
            buffer = new_buffer;

            if bytes_read == 0 {
                return Err(crate::Error::StdIo(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "end of stream reached while reading exact number of bytes",
                )));
            }
        }

        assert_eq!(buffer.len(), len);

        Ok(buffer.consume_all())
    }

    async fn read_at_most_while<F>(
        &mut self,
        len: usize,
        inspect_fn: F,
    ) -> crate::Result<(Sequence, SequenceBuilder)>
    where
        F: FnMut(SequenceBuilderInspector) -> ReadInspectDecision + Send,
    {
        let buffer = self.reserve(len);
        self.read_at_most_into_while(len, buffer, inspect_fn).await
    }

    async fn read_at_most_into_while<F>(
        &mut self,
        len: usize,
        mut into: SequenceBuilder,
        mut inspect_fn: F,
    ) -> crate::Result<(Sequence, SequenceBuilder)>
    where
        F: FnMut(SequenceBuilderInspector) -> ReadInspectDecision + Send,
    {
        while into.len() < len {
            let remaining = len
                .checked_sub(into.len())
                .expect("we validated above that this cannot overflow");

            let (bytes_read, new_into) = self.read_at_most_into(remaining, into).await?;
            into = new_into;

            if bytes_read == 0 {
                return Err(crate::Error::StdIo(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "end of stream reached while reading at most N bytes",
                )));
            }

            match inspect_fn(into.inspect()) {
                ReadInspectDecision::ContinueRead => {}
                ReadInspectDecision::Complete(bytes) => {
                    return Ok((into.consume(bytes), into));
                }
                ReadInspectDecision::Failed(err) => {
                    return Err(err);
                }
            }
        }

        // If we got here, we ran out of our byte budget before we were signaled to end.
        Err(crate::Error::StdIo(io::Error::new(
            ErrorKind::QuotaExceeded,
            "byte budget exceeded while reading from stream",
        )))
    }

    fn into_futures_stream(self) -> Pin<Box<ReadStreamAsFuturesStream<Self>>>
    where
        Self: ReadStream + Sized,
    {
        ReadStreamAsFuturesStream::new(self)
    }
}

/// A decision made by the inspector function in [`ReadStreamExt::read_at_most_while`] or
/// [`ReadStreamExt::read_at_most_into_while`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadInspectDecision {
    /// Continues reading from the stream (assuming there is remaining byte budget - if not, the
    /// read terminates with an error).
    ContinueRead,

    /// Completes reading from the stream, consuming the indicated number of bytes from the buffer
    /// and leaving the remaining bytes (if any) in a `SequenceBuilder` returned alongside the
    /// read data (to potentially be appended to in a follow-up read).
    Complete(usize),

    /// Stops reading from the stream and signals the provided error as the result.
    Failed(crate::Error),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::arithmetic_side_effects, reason = "Not a problem in test code")]

    use std::mem;

    use bytes::Buf;

    use super::*;
    use crate::FakeReadStream;
    use crate::mem::FakeMemoryProvider;
    use crate::testing::async_test;

    const TEST_DATA: &[u8] = b"Hello, world!";

    #[test]
    fn read_at_most_fully_satisfied() {
        async_test! {
            let mut stream = FakeReadStream::new(FakeMemoryProvider::copy_from_static(TEST_DATA));

            let sequence = stream.read_at_most(5).await.unwrap();
            assert_eq!(sequence.len(), 5);
        }
    }

    #[test]
    fn read_at_most_eof_is_partial_read() {
        async_test! {
            let mut stream = FakeReadStream::new(FakeMemoryProvider::copy_from_static(TEST_DATA));

            let sequence = stream.read_at_most(100).await.unwrap();
            assert_eq!(sequence.len(), TEST_DATA.len());
        }
    }

    #[test]
    fn read_exactly_fully_satisfied() {
        async_test! {
            let mut stream = FakeReadStream::new(FakeMemoryProvider::copy_from_static(TEST_DATA));

            let sequence = stream.read_exactly(5).await.unwrap();
            assert_eq!(sequence.len(), 5);
        }
    }

    #[test]
    fn read_exactly_eof_is_error() {
        async_test! {
            let mut stream = FakeReadStream::new(FakeMemoryProvider::copy_from_static(TEST_DATA));

            stream.read_exactly(100).await.unwrap_err();
        };
    }

    #[test]
    fn read_at_most_while_immediate_error() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut inspect_fn_called = false;

            stream.read_at_most_while(100, |_| {
                assert!(!mem::replace(&mut inspect_fn_called, true), "inspect_fn called multiple times");

                ReadInspectDecision::Failed(crate::Error::ContractViolation("oh no".to_string()))
            }).await.unwrap_err();
        }
    }

    #[test]
    fn read_at_most_while_immediate_complete_zero() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let (consumed, remaining) = stream.read_at_most_while(100, |_| {
                ReadInspectDecision::Complete(0)
            }).await.unwrap();

            assert_eq!(consumed.len(), 0);
            assert_eq!(remaining.len(), 2);
        }
    }

    #[test]
    fn read_at_most_while_immediate_complete_partial() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let (consumed, remaining) = stream.read_at_most_while(100, |_| {
                ReadInspectDecision::Complete(1)
            }).await.unwrap();

            assert_eq!(consumed.len(), 1);
            assert_eq!(remaining.len(), 1);
        }
    }

    #[test]
    fn read_at_most_while_immediate_complete_all() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let (consumed, remaining) = stream.read_at_most_while(100, |_| {
                ReadInspectDecision::Complete(2)
            }).await.unwrap();

            assert_eq!(consumed.len(), 2);
            assert_eq!(remaining.len(), 0);
        }
    }

    #[test]
    fn read_at_most_while_deferred_error() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut inspect_fn_called = false;

            stream.read_at_most_while(100, |_| {
                if mem::replace(&mut inspect_fn_called, true) {
                    // This is the second loop ieration - time to act.
                    ReadInspectDecision::Failed(crate::Error::ContractViolation("oh no".to_string()))
                } else {
                    // This is the first loop iteration - do nothing.
                    ReadInspectDecision::ContinueRead
                }
            }).await.unwrap_err();

            assert!(inspect_fn_called);
        }
    }

    #[test]
    fn read_at_most_while_deferred_complete_zero() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut inspect_fn_called = false;

            let (consumed, remaining) = stream.read_at_most_while(100, |_| {
                if mem::replace(&mut inspect_fn_called, true) {
                    // This is the second loop ieration - time to act.
                    ReadInspectDecision::Complete(0)
                } else {
                    // This is the first loop iteration - do nothing.
                    ReadInspectDecision::ContinueRead
                }
            }).await.unwrap();

            assert!(inspect_fn_called);

            assert_eq!(consumed.len(), 0);
            assert_eq!(remaining.len(), 4);
        }
    }

    #[test]
    fn read_at_most_while_deferred_complete_partial() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut inspect_fn_called = false;

            let (consumed, remaining) = stream.read_at_most_while(100, |_| {
                if mem::replace(&mut inspect_fn_called, true) {
                    // This is the second loop ieration - time to act.
                    ReadInspectDecision::Complete(3)
                } else {
                    // This is the first loop iteration - do nothing.
                    ReadInspectDecision::ContinueRead
                }
            }).await.unwrap();

            assert!(inspect_fn_called);

            assert_eq!(consumed.len(), 3);
            assert_eq!(remaining.len(), 1);
        }
    }

    #[test]
    fn read_at_most_while_deferred_complete_all() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut inspect_fn_called = false;

            let (consumed, remaining) = stream.read_at_most_while(100, |_| {
                if mem::replace(&mut inspect_fn_called, true) {
                    // This is the second loop ieration - time to act.
                    ReadInspectDecision::Complete(4)
                } else {
                    // This is the first loop iteration - do nothing.
                    ReadInspectDecision::ContinueRead
                }
            }).await.unwrap();

            assert!(inspect_fn_called);

            assert_eq!(consumed.len(), 4);
            assert_eq!(remaining.len(), 0);
        }
    }

    #[test]
    fn read_at_most_while_eof_is_error() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            stream.read_at_most_while(100, |_| {
                ReadInspectDecision::ContinueRead
            }).await.unwrap_err();
        }
    }

    #[test]
    fn read_at_most_out_of_budget_is_error() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            stream.read_at_most_while(1, |inspector| {
                if inspector.remaining() > 1 {
                    // This payload is over our budget so we should never even see it.
                    unreachable!();
                }

                // The inspector function is never satisfied.
                ReadInspectDecision::ContinueRead
            }).await.unwrap_err();
        }
    }

    #[test]
    fn read_at_most_into_while_nonempty_immediate_error() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut sb = SequenceBuilder::new();
            sb.append(FakeMemoryProvider::copy_from_static(TEST_DATA));

            stream.read_at_most_into_while(100, sb, |_| {
                ReadInspectDecision::Failed(crate::Error::ContractViolation("oh no".to_string()))
            }).await.unwrap_err();
        }
    }

    #[test]
    fn read_at_most_into_while_nonempty_immediate_complete_zero() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut sb = SequenceBuilder::new();
            sb.append(FakeMemoryProvider::copy_from_static(TEST_DATA));

            let (consumed, remaining) = stream.read_at_most_into_while(100, sb, |inspector| {
                assert_eq!(inspector.remaining(), TEST_DATA.len() + 2);

                ReadInspectDecision::Complete(0)
            }).await.unwrap();

            assert_eq!(consumed.len(), 0);
            assert_eq!(remaining.len(), TEST_DATA.len() + 2);
        }
    }

    #[test]
    fn read_at_most_into_while_nonempty_immediate_complete_all() {
        async_test! {
            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeReadStream::with_max_read_size(FakeMemoryProvider::copy_from_static(TEST_DATA), 2);

            let mut sb = SequenceBuilder::new();
            sb.append(FakeMemoryProvider::copy_from_static(TEST_DATA));

            let (consumed, remaining) = stream.read_at_most_into_while(100, sb, |inspector| {
                assert_eq!(inspector.remaining(), TEST_DATA.len() + 2);

                ReadInspectDecision::Complete(inspector.remaining())
            }).await.unwrap();

            assert_eq!(consumed.len(), TEST_DATA.len() + 2);
            assert_eq!(remaining.len(), 0);
        }
    }
}