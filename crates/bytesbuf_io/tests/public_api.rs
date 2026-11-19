// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Byte I/O contracts through public traits and test utilities.

#[cfg(feature = "test-util")]
mod read_ext {
    #![allow(clippy::arithmetic_side_effects, reason = "Not a problem in test code")]

    use std::mem;

    use bytesbuf::mem::GlobalPool;
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::testing::FakeRead;
    use bytesbuf_io::{ReadExt, ReadInspectDecision};
    use new_zealand::nz;
    use testing_aids::async_test;

    const TEST_DATA: &[u8] = b"Hello, world!";

    #[test]
    fn read_at_most_fully_satisfied() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            let data = stream.read_at_most(5).await.unwrap();
            assert_eq!(data.len(), 5);
        });
    }

    #[test]
    fn read_at_most_eof_is_partial_read() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            let data = stream.read_at_most(100).await.unwrap();
            assert_eq!(data.len(), TEST_DATA.len());
        });
    }

    #[test]
    fn read_exactly_fully_satisfied() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            let data = stream.read_exactly(5).await.unwrap();
            assert_eq!(data.len(), 5);
        });
    }

    #[test]
    fn read_exactly_eof_is_error() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            stream.read_exactly(100).await.unwrap_err();
        });
    }

    #[test]
    fn read_at_most_while_immediate_error() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let mut inspect_fn_called = false;

            stream
                .read_at_most_while(100, |_| {
                    assert!(!mem::replace(&mut inspect_fn_called, true), "inspect_fn called multiple times");

                    ReadInspectDecision::Failed(Box::new(std::io::Error::other("oh no")))
                })
                .await
                .unwrap_err();
        });
    }

    #[test]
    fn read_at_most_while_immediate_complete_zero() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let (consumed, remaining) = stream.read_at_most_while(100, |_| ReadInspectDecision::Complete(0)).await.unwrap();

            assert_eq!(consumed.len(), 0);
            assert_eq!(remaining.len(), 2);
        });
    }

    #[test]
    fn read_at_most_while_immediate_complete_partial() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let (consumed, remaining) = stream.read_at_most_while(100, |_| ReadInspectDecision::Complete(1)).await.unwrap();

            assert_eq!(consumed.len(), 1);
            assert_eq!(remaining.len(), 1);
        });
    }

    #[test]
    fn read_at_most_while_immediate_complete_all() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let (consumed, remaining) = stream.read_at_most_while(100, |_| ReadInspectDecision::Complete(2)).await.unwrap();

            assert_eq!(consumed.len(), 2);
            assert_eq!(remaining.len(), 0);
        });
    }

    #[test]
    fn read_at_most_while_deferred_error() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let mut inspect_fn_called = false;

            stream
                .read_at_most_while(100, |_| {
                    if mem::replace(&mut inspect_fn_called, true) {
                        // This is the second loop iteration - time to act.
                        ReadInspectDecision::Failed(Box::new(std::io::Error::other("oh no")))
                    } else {
                        // This is the first loop iteration - do nothing.
                        ReadInspectDecision::ContinueRead
                    }
                })
                .await
                .unwrap_err();

            assert!(inspect_fn_called);
        });
    }

    #[test]
    fn read_at_most_while_deferred_complete_zero() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let mut inspect_fn_called = false;

            let (consumed, remaining) = stream
                .read_at_most_while(100, |_| {
                    if mem::replace(&mut inspect_fn_called, true) {
                        // This is the second loop ieration - time to act.
                        ReadInspectDecision::Complete(0)
                    } else {
                        // This is the first loop iteration - do nothing.
                        ReadInspectDecision::ContinueRead
                    }
                })
                .await
                .unwrap();

            assert!(inspect_fn_called);

            assert_eq!(consumed.len(), 0);
            assert_eq!(remaining.len(), 4);
        });
    }

    #[test]
    fn read_at_most_while_deferred_complete_partial() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let mut inspect_fn_called = false;

            let (consumed, remaining) = stream
                .read_at_most_while(100, |_| {
                    if mem::replace(&mut inspect_fn_called, true) {
                        // This is the second loop ieration - time to act.
                        ReadInspectDecision::Complete(3)
                    } else {
                        // This is the first loop iteration - do nothing.
                        ReadInspectDecision::ContinueRead
                    }
                })
                .await
                .unwrap();

            assert!(inspect_fn_called);

            assert_eq!(consumed.len(), 3);
            assert_eq!(remaining.len(), 1);
        });
    }

    #[test]
    fn read_at_most_while_deferred_complete_all() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            let mut inspect_fn_called = false;

            let (consumed, remaining) = stream
                .read_at_most_while(100, |_| {
                    if mem::replace(&mut inspect_fn_called, true) {
                        // This is the second loop iteration - time to act.
                        ReadInspectDecision::Complete(4)
                    } else {
                        // This is the first loop iteration - do nothing.
                        ReadInspectDecision::ContinueRead
                    }
                })
                .await
                .unwrap();

            assert!(inspect_fn_called);

            assert_eq!(consumed.len(), 4);
            assert_eq!(remaining.len(), 0);
        });
    }

    #[test]
    fn read_at_most_while_eof_is_error() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            stream
                .read_at_most_while(100, |_| ReadInspectDecision::ContinueRead)
                .await
                .unwrap_err();
        });
    }

    #[test]
    fn read_at_most_out_of_budget_is_error() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents).max_read_size(nz!(2)).build();

            stream
                .read_at_most_while(1, |inspector| {
                    if inspector.len() > 1 {
                        // This payload is over our budget so we should never even see it.
                        unreachable!();
                    }

                    // The inspector function is never satisfied.
                    ReadInspectDecision::ContinueRead
                })
                .await
                .unwrap_err();
        });
    }

    #[test]
    fn read_at_most_into_while_nonempty_immediate_error() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents.clone()).max_read_size(nz!(2)).build();

            let mut buf = BytesBuf::new();
            buf.put_bytes(contents);

            stream
                .read_at_most_into_while(100, buf, |_| ReadInspectDecision::Failed(Box::new(std::io::Error::other("oh no"))))
                .await
                .unwrap_err();
        });
    }

    #[test]
    fn read_at_most_into_while_nonempty_immediate_complete_zero() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents.clone()).max_read_size(nz!(2)).build();

            let mut buf = BytesBuf::new();
            buf.put_bytes(contents);

            let (consumed, remaining) = stream
                .read_at_most_into_while(100, buf, |inspector| {
                    assert_eq!(inspector.len(), TEST_DATA.len() + 2);

                    ReadInspectDecision::Complete(0)
                })
                .await
                .unwrap();

            assert_eq!(consumed.len(), 0);
            assert_eq!(remaining.len(), TEST_DATA.len() + 2);
        });
    }

    #[test]
    fn read_at_most_into_while_nonempty_immediate_complete_all() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            // Read up to 2 bytes at a time, to force read_at_most_while() to loop (if appropriate).
            let mut stream = FakeRead::builder().contents(contents.clone()).max_read_size(nz!(2)).build();

            let mut buf = BytesBuf::new();
            buf.put_bytes(contents);

            let (consumed, remaining) = stream
                .read_at_most_into_while(100, buf, |inspector| {
                    assert_eq!(inspector.len(), TEST_DATA.len() + 2);

                    ReadInspectDecision::Complete(inspector.len())
                })
                .await
                .unwrap();

            assert_eq!(consumed.len(), TEST_DATA.len() + 2);
            assert_eq!(remaining.len(), 0);
        });
    }

    #[test]
    #[should_panic]
    fn read_at_most_into_while_panics_when_buffer_already_at_len() {
        async_test(async || {
            let memory = GlobalPool::new();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);

            let mut stream = FakeRead::builder().contents(contents.clone()).build();

            // Create a buffer that already contains exactly `len` bytes
            let mut buf = BytesBuf::new();
            buf.put_bytes(contents);
            let buffer_len = buf.len();

            // This should panic because .len() == len
            _ = stream
                .read_at_most_into_while(buffer_len, buf, |_| ReadInspectDecision::ContinueRead)
                .await;
        });
    }
}

#[cfg(all(feature = "test-util", feature = "futures-stream"))]
mod read_futures {
    use std::convert::Infallible;
    use std::future::ready;
    use std::pin::{Pin, pin};
    use std::task::Waker;
    use std::{fmt, task};

    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared};
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::testing::{FakeRead, Pending};
    use bytesbuf_io::{ReadAsFuturesStream, ReadExt};
    use futures::{Stream, StreamExt};
    use new_zealand::nz;
    use testing_aids::{YieldFuture, async_test};

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
            let memory = GlobalPool::new();
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

#[cfg(feature = "test-util")]
mod write_ext {
    use std::convert::Infallible;

    use bytesbuf::BytesView;
    use bytesbuf_io::WriteExt;
    use bytesbuf_io::testing::FakeWrite;
    use testing_aids::async_test;

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
                    Err(std::io::Error::other("oopsy-woopsie, serialization failed"))
                })
                .await
                .unwrap_err();

            let contents = stream.into_contents();

            assert_eq!(contents.len(), 0);
        });
    }
}

#[cfg(feature = "test-util")]
mod testing_fake_read {
    use bytesbuf::BytesView;
    use bytesbuf::mem::{GlobalPool, Memory};
    use bytesbuf_io::ReadExt;
    use bytesbuf_io::testing::FakeRead;
    use new_zealand::nz;
    use testing_aids::async_test;

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

    #[test]
    fn memory_returns_configured_provider() {
        use bytesbuf::mem::testing::TransparentMemory;
        use bytesbuf::mem::{CallbackMemory, OpaqueMemory};

        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let memory = GlobalPool::new();
        let contents = BytesView::copied_from_slice(b"test", &memory);
        let read_stream = FakeRead::builder().contents(contents).memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = read_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }
}

#[cfg(feature = "test-util")]
mod testing_fake_write {
    use std::convert::Infallible;

    use bytesbuf::BytesView;
    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{CallbackMemory, Memory, OpaqueMemory};
    use bytesbuf_io::WriteExt;
    use bytesbuf_io::testing::FakeWrite;
    use testing_aids::async_test;

    #[test]
    fn smoke_test() {
        async_test(async || {
            let mut write_stream = FakeWrite::new();

            write_stream
                .prepare_and_write(1234, |mut buf| {
                    buf.put_byte(1);
                    buf.put_byte(2);
                    buf.put_byte(3);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(write_stream.contents().len(), 3);

            let mut contents = write_stream.into_contents();
            assert_eq!(contents.len(), 3);

            assert_eq!(contents.get_byte(), 1);
            assert_eq!(contents.get_byte(), 2);
            assert_eq!(contents.get_byte(), 3);
            assert_eq!(contents.len(), 0);
        });
    }

    #[test]
    fn default_returns_working_instance() {
        async_test(async || {
            let mut write_stream = FakeWrite::default();

            write_stream
                .prepare_and_write(10, |mut buf| {
                    buf.put_byte(42);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(write_stream.contents().len(), 1);

            let mut contents = write_stream.into_contents();
            assert_eq!(contents.get_byte(), 42);
        });
    }

    #[test]
    fn memory_returns_configured_provider() {
        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let write_stream = FakeWrite::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = write_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }
}

#[cfg(feature = "test-util")]
mod testing_null {
    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{CallbackMemory, Memory, OpaqueMemory};
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::testing::Null;
    use testing_aids::execute_or_terminate_process;

    #[test]
    fn smoke_test() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = Null::new();

                let buffer = s.reserve(1000);
                assert!(buffer.remaining_capacity() >= 1000);

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

    #[test]
    fn default_returns_working_instance() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = Null::default();

                // Verify it can reserve memory
                let buffer = s.reserve(100);
                assert!(buffer.remaining_capacity() >= 100);

                // Verify read operations work
                let (bytes_read, _) = s.read_at_most_into(10, BytesBuf::new()).await.unwrap();
                assert_eq!(bytes_read, 0);

                // Verify write operations work
                let empty_view = BytesView::default();
                s.write(empty_view).await.unwrap();
            });
        });
    }

    #[test]
    fn memory_returns_configured_provider() {
        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let null_stream = Null::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = null_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }
}

#[cfg(feature = "test-util")]
mod testing_pending {
    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{CallbackMemory, Memory, OpaqueMemory};
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::testing::Pending;
    use future_assert::FutureTestExt;

    #[test]
    fn smoke_test() {
        let stream = Pending::new();

        let reserved1 = stream.reserve(123);
        assert!(reserved1.capacity() >= 123);

        let memory = stream.memory();
        let reserved2 = memory.reserve(123);
        assert!(reserved2.capacity() >= 123);
    }

    #[test]
    fn default_returns_working_instance() {
        let stream = Pending::default();

        // Verify it can reserve memory
        let buffer = stream.reserve(100);
        assert!(buffer.remaining_capacity() >= 100);

        // Verify memory() works
        let stream_memory = stream.memory();
        let buffer2 = stream_memory.reserve(50);
        assert!(buffer2.remaining_capacity() >= 50);
    }

    #[test]
    fn memory_returns_configured_provider() {
        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let pending_stream = Pending::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = pending_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }

    #[test]
    fn read_at_most_into_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let buffer = BytesBuf::new();

        stream.read_at_most_into(100, buffer).unwrap_pending_for(1);
    }

    #[test]
    fn read_more_into_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let buffer = BytesBuf::new();

        stream.read_more_into(buffer).unwrap_pending_for(1);
    }

    #[test]
    fn read_any_returns_pending_on_first_poll() {
        let mut stream = Pending::new();

        stream.read_any().unwrap_pending_for(1);
    }

    #[test]
    fn write_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let data = BytesView::default();

        stream.write(data).unwrap_pending_for(1);
    }
}
