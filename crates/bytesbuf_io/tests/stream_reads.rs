// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stream reads through public APIs.

#![cfg(test)]

use bytesbuf::mem::GlobalPool;
use std::sync::LazyLock;
#[cfg(feature = "futures-stream")]
static MEMORY: LazyLock<GlobalPool> = LazyLock::new(GlobalPool::new);

#[cfg(feature = "futures-stream")]
mod read_fixture;

#[cfg(feature = "futures-stream")]
mod read_futures {
    use std::convert::Infallible;
    use std::future::ready;
    use std::pin::{Pin, pin};
    use std::task::Waker;
    use std::{fmt, task};

    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::{Read, ReadAsFuturesStream, ReadExt};
    use futures::{Stream, StreamExt};
    use new_zealand::nz;
    use testing_aids::{YieldFuture, async_test};

    use crate::read_fixture::FakeRead;

    #[derive(Debug)]
    struct Pending;

    impl Pending {
        fn new() -> Self {
            Self
        }
    }

    impl Memory for Pending {
        fn reserve(&self, min_bytes: usize) -> BytesBuf {
            TransparentMemory::new().reserve(min_bytes)
        }
    }

    impl HasMemory for Pending {
        fn memory(&self) -> impl MemoryShared {
            TransparentMemory::new()
        }
    }

    impl Read for Pending {
        type Error = Infallible;

        async fn read_at_most_into(&mut self, _: usize, _: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
            std::future::pending().await
        }

        async fn read_more_into(&mut self, _: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
            std::future::pending().await
        }

        async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
            std::future::pending().await
        }
    }

    #[test]
    fn smoke_test() {
        async_test(async || {
            let memory = crate::MEMORY.clone();
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

    impl bytesbuf_io::Read for YieldThenRead {
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
    fn formatting_pending_read_preserves_the_active_borrow() {
        let contents = BytesView::copied_from_slice(b"Hello", &TransparentMemory::new());
        let inner = YieldThenRead {
            inner: FakeRead::builder().contents(contents).build(),
        };
        let mut stream = inner.into_futures_stream();
        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(stream.as_mut().poll_next(&mut cx).is_pending());
        let pending_debug = format!("{stream:?}");
        assert!(pending_debug.contains("active_read.is_some(): true"));
        assert!(!pending_debug.contains("inner:"));

        let task::Poll::Ready(Some(Ok(mut data))) = stream.as_mut().poll_next(&mut cx) else {
            panic!("the same read must complete after its first yield");
        };
        assert_eq!(data.len(), 5);
        for expected in b"Hello" {
            assert_eq!(data.get_byte(), *expected);
        }
        assert!(format!("{stream:?}").contains("inner:"));
    }

    #[test]
    fn pending_on_first_poll_then_returns_result() {
        async_test(async || {
            let memory = crate::MEMORY.clone();
            let contents = BytesView::copied_from_slice(b"Hello", &memory);
            let inner = YieldThenRead {
                inner: FakeRead::builder().contents(contents).build(),
            };

            let mut futures_stream = inner.into_futures_stream();

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

    impl bytesbuf_io::Read for ErroringRead {
        type Error = TestError;

        fn read_at_most_into(&mut self, _len: usize, _into: BytesBuf) -> impl Future<Output = Result<(usize, BytesBuf), Self::Error>> {
            ready(Err(TestError("read_at_most_into error".to_string())))
        }

        fn read_more_into(&mut self, _into: BytesBuf) -> impl Future<Output = Result<(usize, BytesBuf), Self::Error>> {
            ready(Err(TestError("read_more_into error".to_string())))
        }

        fn read_any(&mut self) -> impl Future<Output = Result<BytesBuf, Self::Error>> {
            ready(Err(TestError("read_any error".to_string())))
        }
    }

    #[test]
    fn passes_through_error_from_inner() {
        async_test(async || {
            let inner = ErroringRead::default();
            let mut futures_stream = inner.into_futures_stream();

            let result = futures_stream.next().await;

            match result {
                Some(Err(TestError(msg))) => {
                    assert_eq!(msg, "read_any error");
                }
                _ => panic!("Expected Some(Err(TestError(_)))"),
            }
        });
    }

    #[test]
    fn stream_is_send_when_source_is_send() {
        const fn assert_send<T: Send>() {}

        // The `Read` contract requires `Send` from both the implementation and any returned
        // futures, so the adapter stream must remain `Send` for any `Send` source.
        assert_send::<Pin<Box<ReadAsFuturesStream<FakeRead>>>>();
    }
}
