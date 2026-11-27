// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Read operations through public APIs.

#![cfg(test)]

fn memory() -> &'static bytesbuf::mem::GlobalPool {
    static MEMORY: std::sync::OnceLock<bytesbuf::mem::GlobalPool> = std::sync::OnceLock::new();

    MEMORY.get_or_init(bytesbuf::mem::GlobalPool::new)
}

mod read_fixture;

mod read_ext {
    #![allow(clippy::arithmetic_side_effects, reason = "Not a problem in test code")]

    use std::mem;

    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::{ReadExt, ReadInspectDecision};
    use new_zealand::nz;
    use testing_aids::async_test;

    use crate::read_fixture::FakeRead;

    const TEST_DATA: &[u8] = b"Hello, world!";

    #[test]
    fn read_at_most_fully_satisfied() {
        async_test(async || {
            let memory = crate::memory().clone();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            let data = stream.read_at_most(5).await.unwrap();
            assert_eq!(data.len(), 5);
        });
    }

    #[test]
    fn read_at_most_eof_is_partial_read() {
        async_test(async || {
            let memory = crate::memory().clone();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            let data = stream.read_at_most(100).await.unwrap();
            assert_eq!(data.len(), TEST_DATA.len());
        });
    }

    #[test]
    fn read_exactly_fully_satisfied() {
        async_test(async || {
            let memory = crate::memory().clone();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            let data = stream.read_exactly(5).await.unwrap();
            assert_eq!(data.len(), 5);
        });
    }

    #[test]
    fn read_exactly_eof_is_error() {
        async_test(async || {
            let memory = crate::memory().clone();
            let contents = BytesView::copied_from_slice(TEST_DATA, &memory);
            let mut stream = FakeRead::new(contents);

            stream.read_exactly(100).await.unwrap_err();
        });
    }

    #[test]
    fn read_at_most_while_immediate_error() {
        async_test(async || {
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
            let memory = crate::memory().clone();
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
