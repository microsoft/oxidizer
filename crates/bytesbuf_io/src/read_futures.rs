// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::Poll::{Pending, Ready};
use std::{mem, task};

use bytesbuf::{BytesBuf, BytesView};

use crate::Read;

/// Adapts a [`Read`] implementation to the `futures::Stream` API.
///
/// Each item in the `futures::Stream` is a [`BytesView`] containing some bytes read
/// from the underlying [`Read`].
///
/// # Security
///
/// **This adapter is insecure if the side producing the bytes is not trusted**. An attacker
/// may trickle data byte-by-byte, consuming a large amount of resources.
///
/// Robust code working with untrusted sources should take precautions such as only processing
/// read data when either a time or length threshold is reached and reusing buffers that
/// have remaining capacity, appending additional data to existing buffers using
/// [`read_more_into()`][crate::Read::read_more_into] instead of reserving new buffers
/// for each read operation.
pub struct ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    // References `inner`. Must be defined above `inner` to ensure it gets dropped first.`
    #[expect(clippy::type_complexity, reason = "never needs to be named, good enough")]
    active_read: Option<Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>>>>>,

    // Safety invariant: we can only touch this field if `active_read` is `None`.
    inner: S,

    // This struct must remain pinned because `active_read` references `inner`.
    _require_pin: PhantomPinned,
}

impl<S> ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    pub(crate) fn new(inner: S) -> Pin<Box<Self>> {
        Box::pin(Self {
            active_read: None,
            inner,
            _require_pin: PhantomPinned,
        })
    }

    /// Abandons any ongoing read operation and returns the source.
    #[must_use]
    pub fn into_inner(self: Pin<Box<Self>>) -> S {
        // SAFETY: We are going to unpin `self` by first dropping `active_read`, which is the thing
        // that references `inner` and requires pinning. Once `active_read` has been dropped,
        // no more pinning requirements exist.
        let mut unpinned = unsafe { Pin::into_inner_unchecked(self) };

        unpinned.active_read = None;
        unpinned.inner
    }
}

impl<S> futures::Stream for ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    type Item = Result<BytesView, S::Error>;

    fn poll_next<'a>(self: Pin<&'a mut Self>, cx: &'a mut task::Context) -> task::Poll<Option<Self::Item>> {
        // SAFETY: We are not moving `inner`, which is the field that must remain pinned.
        let this = unsafe { self.get_unchecked_mut() };

        let mut active_read = if let Some(active_read) = this.active_read.take() {
            active_read
        } else {
            let inner = &mut this.inner;
            let future = async move { inner.read_any().await };
            let boxed_future = Box::pin(future);

            // SAFETY: We overwrite the lifetime of the future to 'static because in reality we
            // have a lifetime bounded to the lifetime of the struct itself but this cannot be
            // meaningfully expressed in Rust, so we have to expand it to 'static. For safety, we
            // have to ensure that the future does not outlive either the struct instance itself
            // or `inner`, and that we do not touch `inner` while the future exists.
            unsafe {
                mem::transmute::<
                    Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>> + 'a>>,
                    Pin<Box<dyn Future<Output = Result<BytesBuf, S::Error>>>>,
                >(boxed_future)
            }
        };

        let result = active_read.as_mut().poll(cx);

        match result {
            Ready(Ok(mut buf)) => {
                let data = buf.consume_all();

                if data.is_empty() {
                    // We have reached the end of the stream.
                    return Ready(None);
                }

                Ready(Some(Ok(data)))
            }
            Ready(Err(e)) => Ready(Some(Err(e))),
            Pending => {
                this.active_read = Some(active_read);
                Pending
            }
        }
    }
}

impl<S> Debug for ReadAsFuturesStream<S>
where
    S: Read + Debug,
{
    #[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
    #[cfg_attr(test, mutants::skip)] // We have no contract to test.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("active_read.is_some()", &self.active_read.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::convert::Infallible;
    use std::pin::pin;
    use std::task::Waker;

    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared};
    use futures::{Stream, StreamExt};
    use new_zealand::nz;
    use testing_aids::{YieldFuture, async_test};

    use super::*;
    use crate::ReadExt;
    use crate::testing::{FakeRead, Pending};

    #[test]
    fn smoke_test() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(b"Hello, w", &memory);
            let inner = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let mut futures_stream = inner.into_futures_stream();

            // It can be read from.
            let mut payload1 = futures_stream.next().await.unwrap().unwrap();
            assert_eq!(payload1.len(), 2);
            assert_eq!(payload1.get_byte(), b'H');
            assert_eq!(payload1.get_byte(), b'e');

            let mut payload2 = futures_stream.next().await.unwrap().unwrap();
            assert_eq!(payload2.len(), 2);
            assert_eq!(payload2.get_byte(), b'l');
            assert_eq!(payload2.get_byte(), b'l');

            // We can get back the original.
            let mut original = futures_stream.into_inner();

            let mut payload3 = original.read_exactly(2).await.unwrap();
            assert_eq!(payload3.len(), 2);
            assert_eq!(payload3.get_byte(), b'o');
            assert_eq!(payload3.get_byte(), b',');

            // Back to the futures::Stream!
            let mut futures_stream = original.into_futures_stream();

            let mut payload4 = futures_stream.next().await.unwrap().unwrap();
            assert_eq!(payload4.len(), 2);
            assert_eq!(payload4.get_byte(), b' ');
            assert_eq!(payload4.get_byte(), b'w');

            // And once we hit end of stream, it needs to return `None`.
            assert!(futures_stream.next().await.is_none());
        });
    }

    #[test]
    fn pending_read_cancelled_on_into_inner() {
        let inner = Pending::new();

        let mut futures_stream = inner.into_futures_stream();

        // We can cancel a pending read. Need to test this directly against the impl,
        // as the `futures::Stream` extension methods are lazy and will not start the read.
        // let mut futures_stream = original.into_futures_stream();

        let mut cx = task::Context::from_waker(Waker::noop());
        assert!(matches!(futures_stream.as_mut().poll_next(&mut cx), task::Poll::Pending));

        // The inner stream is not capable of completing reads, so a bit hard to test.
        // Well, as long as there is no panic or Miri complaint, we can be satisfied.
        let mut inner = futures_stream.into_inner();

        let read_future = pin!(inner.read_any());
        assert!(read_future.poll(&mut cx).is_pending());
    }

    /// A Read implementation that yields on first poll then returns data.
    /// This is used to test that `ReadAsFuturesStream` correctly handles `Poll::Pending`.
    #[derive(Debug)]
    struct YieldThenRead {
        inner: FakeRead,
    }

    impl Memory for YieldThenRead {
        fn reserve(&self, min_bytes: usize) -> BytesBuf {
            self.inner.reserve(min_bytes)
        }
    }

    impl HasMemory for YieldThenRead {
        fn memory(&self) -> impl MemoryShared {
            self.inner.memory()
        }
    }

    impl crate::Read for YieldThenRead {
        type Error = Infallible;

        async fn read_at_most_into(&mut self, len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
            YieldFuture::default().await;
            self.inner.read_at_most_into(len, into).await
        }

        async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
            YieldFuture::default().await;
            self.inner.read_more_into(into).await
        }

        async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
            YieldFuture::default().await;
            self.inner.read_any().await
        }
    }

    #[test]
    fn pending_on_first_poll_then_returns_result() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(b"Hello", &memory);
            let inner = YieldThenRead {
                inner: FakeRead::builder().contents(contents).build(),
            };

            let mut futures_stream = ReadAsFuturesStream::new(inner);

            // First poll should be Pending due to YieldFuture
            let waker = Waker::noop();
            let mut cx = task::Context::from_waker(waker);
            let poll_result = futures_stream.as_mut().poll_next(&mut cx);
            assert!(matches!(poll_result, task::Poll::Pending));

            // Second poll should return the actual data
            let poll_result = futures_stream.as_mut().poll_next(&mut cx);
            if let task::Poll::Ready(Some(Ok(mut data))) = poll_result {
                assert_eq!(data.len(), 5);
                assert_eq!(data.get_byte(), b'H');
                assert_eq!(data.get_byte(), b'e');
                assert_eq!(data.get_byte(), b'l');
                assert_eq!(data.get_byte(), b'l');
                assert_eq!(data.get_byte(), b'o');
            } else {
                panic!("Expected Ready(Some(Ok(_)))");
            }
        });
    }

    /// A Read implementation that always returns an error.
    #[derive(Debug)]
    struct ErroringRead {
        memory: TransparentMemory,
    }

    impl Default for ErroringRead {
        fn default() -> Self {
            Self {
                memory: TransparentMemory::new(),
            }
        }
    }

    #[derive(Debug)]
    struct TestError(String);

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for TestError {}

    impl Memory for ErroringRead {
        fn reserve(&self, min_bytes: usize) -> BytesBuf {
            self.memory.reserve(min_bytes)
        }
    }

    impl HasMemory for ErroringRead {
        fn memory(&self) -> impl MemoryShared {
            self.memory.clone()
        }
    }

    impl crate::Read for ErroringRead {
        type Error = TestError;

        async fn read_at_most_into(&mut self, _len: usize, _into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
            Err(TestError("read_at_most_into error".to_string()))
        }

        async fn read_more_into(&mut self, _into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
            Err(TestError("read_more_into error".to_string()))
        }

        async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
            Err(TestError("read_any error".to_string()))
        }
    }

    #[test]
    fn passes_through_error_from_inner() {
        async_test(async || {
            let inner = ErroringRead::default();
            let mut futures_stream = ReadAsFuturesStream::new(inner);

            let result = futures_stream.next().await;

            match result {
                Some(Err(TestError(msg))) => {
                    assert_eq!(msg, "read_any error");
                }
                _ => panic!("Expected Some(Err(TestError(_)))"),
            }
        });
    }
}
