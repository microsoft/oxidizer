// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::{BytesBuf, BytesView};

use crate::Write;

/// Universal convenience methods built on top of [`Write`].
///
/// This trait is implemented for all types that implement [`Write`].
#[trait_variant::make(Send)]
pub trait WriteExt: Write {
    /// Provides a memory buffer to a callback, then writes the contents.
    ///
    /// The write is only executed if `prepare_fn` returns `Ok(BytesView)` with the data to write.
    ///
    /// The expectation is that you perform your serialization logic directly in `prepare_fn`,
    /// instead of just copying bytes from some other buffer (which would not be efficient).
    ///
    /// # Example
    ///
    /// ```
    /// # testing_aids::execute_or_terminate_process(|| futures::executor::block_on(async {
    /// # use bytesbuf_io::testing::Null;
    /// use std::convert::Infallible;
    ///
    /// use bytesbuf_io::WriteExt;
    ///
    /// # fn get_sink() -> Null { Null::new() }
    /// let mut sink = get_sink();
    ///
    /// sink.prepare_and_write(100, |mut buf| {
    ///     buf.put_slice(*b"Hello, world!");
    ///     Ok::<_, Infallible>(buf.consume_all())
    /// })
    /// .await
    /// .unwrap();
    /// # }));
    /// ```
    async fn prepare_and_write<F, E>(&mut self, min_capacity: usize, prepare_fn: F) -> Result<(), crate::Error>
    where
        F: FnOnce(BytesBuf) -> Result<BytesView, E> + Send,
        E: std::error::Error + Send + Sync + 'static;
}

impl<T> WriteExt for T
where
    T: Write,
{
    async fn prepare_and_write<F, E>(&mut self, min_capacity: usize, prepare_fn: F) -> Result<(), crate::Error>
    where
        F: FnOnce(BytesBuf) -> Result<BytesView, E> + Send,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.write(prepare_fn(self.reserve(min_capacity)).map_err(crate::Error::caused_by)?)
            .await
            .map_err(crate::Error::caused_by)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use testing_aids::async_test;

    use super::*;
    use crate::testing::FakeWrite;

    const TEST_DATA: &[u8] = b"Hello, world!";

    #[test]
    fn prepare_and_write_writes_on_prepare_success() {
        async_test(async || {
            let mut stream = FakeWrite::new();

            stream
                .prepare_and_write(100, |mut buf| {
                    assert!(buf.capacity() >= 100);

                    buf.put_slice(TEST_DATA);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            let contents = stream.into_contents();

            assert_eq!(contents.len(), TEST_DATA.len());
        });
    }

    #[test]
    fn prepare_and_write_cancels_on_prepare_fail() {
        async_test(async || {
            let mut stream = FakeWrite::new();

            stream
                .prepare_and_write(100, |mut buf| {
                    assert!(buf.capacity() >= 100);

                    buf.put_slice(TEST_DATA);
                    Err(crate::Error::caused_by("oopsy-woopsie, serialization failed"))
                })
                .await
                .unwrap_err();

            let contents = stream.into_contents();

            assert_eq!(contents.len(), 0);
        });
    }
}
