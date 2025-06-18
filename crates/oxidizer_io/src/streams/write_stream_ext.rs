// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::BufMut;

use crate::WriteStream;
use crate::mem::{Sequence, SequenceBuilder};

/// Convenience methods built on top of [`WriteStream`] to make it easier to write to a stream
/// under different conditions.
///
/// This trait is implemented for all types that implement [`WriteStream`].
#[trait_variant::make(Send)]
pub trait WriteStreamExt {
    /// Prepares a sequence of bytes to be written to the stream, reserving at least `min_capacity`
    /// bytes of I/O memory capacity to store the prepared bytes in.
    ///
    /// The write is only executed if `prepare_fn` returns `Ok(Sequence)` with the data to write.
    ///
    /// The expectation is that you perform your serialization logic directly in `prepare_fn`,
    /// instead of just copying bytes from some other buffer (which would not be efficient).
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::NullStream;
    /// use bytes::BufMut;
    /// use oxidizer_io::WriteStreamExt;
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// stream.prepare_and_write(100, |mut sequence_builder| {
    ///     sequence_builder.put_slice(b"Hello, world!");
    ///     Ok(sequence_builder.consume_all())
    /// }).await.unwrap();
    /// # }));
    /// ```
    async fn prepare_and_write<F>(
        &mut self,
        min_capacity: usize,
        prepare_fn: F,
    ) -> crate::Result<()>
    where
        F: FnOnce(SequenceBuilder) -> crate::Result<Sequence> + Send;

    /// Copies some bytes to a temporary buffer and writes them to the stream.
    ///
    /// This is inefficient - for optimal performance and efficiency, you should already generate
    /// your data in I/O memory using a `SequenceBuilder` instead of using arbitrary Rust memory.
    ///
    /// # Example
    ///
    /// ```
    /// # oxidizer_testing::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use oxidizer_io::NullStream;
    /// use oxidizer_io::WriteStreamExt;
    ///
    /// # fn get_stream() -> NullStream { NullStream::new() }
    /// let mut stream = get_stream();
    ///
    /// stream.copy_and_write(b"Hello, world!".as_slice()).await.unwrap();
    /// # }));
    async fn copy_and_write(&mut self, data: impl bytes::Buf + Send) -> crate::Result<()>;
}

impl<T> WriteStreamExt for T
where
    T: WriteStream,
{
    async fn prepare_and_write<F>(
        &mut self,
        min_capacity: usize,
        prepare_fn: F,
    ) -> crate::Result<()>
    where
        F: FnOnce(SequenceBuilder) -> crate::Result<Sequence> + Send,
    {
        self.write(prepare_fn(self.reserve(min_capacity))?).await
    }

    async fn copy_and_write(&mut self, data: impl bytes::Buf + Send) -> crate::Result<()> {
        self.prepare_and_write(data.remaining(), move |mut sb| {
            sb.put(data);
            Ok(sb.consume_all())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeWriteStream;
    use crate::testing::async_test;

    const TEST_DATA: &[u8] = b"Hello, world!";

    #[test]
    fn prepare_and_write_writes_on_prepare_success() {
        async_test! {
            let mut stream = FakeWriteStream::new();

            stream
                .prepare_and_write(100, |mut sb| {
                    assert!(sb.capacity() >= 100);

                    sb.put_slice(TEST_DATA);
                    Ok(sb.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(stream.inner().len(), TEST_DATA.len());
        };
    }

    #[test]
    fn prepare_and_write_cancels_on_prepare_fail() {
        async_test! {
            let mut stream = FakeWriteStream::new();

            stream
                .prepare_and_write(100, |mut sb| {
                    assert!(sb.capacity() >= 100);

                    sb.put_slice(TEST_DATA);
                    Err(crate::Error::ContractViolation("oopsy-woopsie, serialization failed".to_string()))
                })
                .await
                .unwrap_err();

            assert_eq!(stream.inner().len(), 0);
        };
    }

    #[test]
    fn copy_and_write() {
        async_test! {
            let mut stream = FakeWriteStream::new();

            stream
                .copy_and_write(TEST_DATA)
                .await
                .unwrap();

            assert_eq!(stream.inner().len(), TEST_DATA.len());

            // Appending 0-byte sequences does nothing but is basically a valid operation.
            stream
                .copy_and_write(b"".as_slice())
                .await
                .unwrap();

            assert_eq!(stream.inner().len(), TEST_DATA.len());
        }
    }
}