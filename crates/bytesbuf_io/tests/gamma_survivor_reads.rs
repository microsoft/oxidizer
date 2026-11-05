// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests in-memory reads, read errors, and pending operations.

#![cfg(all(test, feature = "test-util"))]

#[cfg(feature = "futures-stream")]
use std::future::pending;
use std::num::NonZero;
use std::pin::pin;
#[cfg(feature = "futures-stream")]
use std::task::Poll;
use std::task::{Context, Waker};

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};
use bytesbuf_io::testing::{FakeRead, Null, Pending};
use bytesbuf_io::{Read, ReadExt, ReadInspectDecision, Write};
use futures::executor::block_on;

#[derive(Debug, Default)]
struct GammaSurvivorErrorRead;

impl Memory for GammaSurvivorErrorRead {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        TransparentMemory::new().reserve(min_bytes)
    }
}

impl HasMemory for GammaSurvivorErrorRead {
    fn memory(&self) -> impl MemoryShared {
        TransparentMemory::new()
    }
}

impl Read for GammaSurvivorErrorRead {
    type Error = std::io::Error;

    async fn read_at_most_into(&mut self, _: usize, _: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        Err(std::io::Error::other("gamma survivor scripted failure"))
    }

    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        self.read_at_most_into(1, into).await
    }

    async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
        self.read_at_most_into(1, BytesBuf::new()).await.map(|(_, buffer)| buffer)
    }
}

#[test]
fn gamma_survivor_exact_and_conditional_errors_preserve_cause() {
    block_on(async {
        let mut source = GammaSurvivorErrorRead;
        let error = source.read_exactly(2).await.unwrap_err();
        assert!(format!("{error:?}").contains("gamma survivor scripted failure"));
        let error = source
            .read_at_most_while(2, |_| ReadInspectDecision::Complete(1))
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("gamma survivor scripted failure"));
        let mut source = Null::new();
        assert!(source.read_exactly(0).await.unwrap().is_empty());
        let error = source.read_exactly(1).await.unwrap_err();
        assert!(format!("{error:?}").contains("source was closed before exact number of bytes could be read"));
        let error = source
            .read_at_most_while(1, |_| ReadInspectDecision::ContinueRead)
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("source was closed before conditional read was completed"));
    });
}

#[test]
fn gamma_survivor_partial_reads_inspection_and_budget_boundaries() {
    block_on(async {
        let mut source = FakeRead::builder()
            .contents(BytesView::from(b"abcdefgh".to_vec()))
            .max_read_size(NonZero::new(1).unwrap())
            .build();
        assert_eq!(source.read_exactly(3).await.unwrap(), b"abc");
        let mut seen = Vec::new();
        let (prefix, mut remaining) = source
            .read_at_most_while(4, |view| {
                seen.push(view.to_vec());
                if view.len() == 3 {
                    ReadInspectDecision::Complete(2)
                } else {
                    ReadInspectDecision::ContinueRead
                }
            })
            .await
            .unwrap();
        assert_eq!(seen, [b"d".to_vec(), b"de".to_vec(), b"def".to_vec()]);
        assert_eq!(prefix, b"de");
        assert_eq!(remaining.consume_all(), b"f");
        assert_eq!(source.read_exactly(2).await.unwrap(), b"gh");

        let mut source = FakeRead::new(BytesView::from(b"abcdef".to_vec()));
        let mut inspected = 0;
        let error = source
            .read_at_most_while(3, |view| {
                inspected += 1;
                assert_eq!(view, b"abc");
                ReadInspectDecision::ContinueRead
            })
            .await
            .unwrap_err();
        assert_eq!(inspected, 1);
        assert!(format!("{error:?}").contains("byte budget exceeded while performing conditional read"));
    });
}

#[test]
fn gamma_survivor_one_byte_conditional_read_is_not_eof() {
    block_on(async {
        let mut source = FakeRead::new(BytesView::from(b"x".to_vec()));
        let (view, buffer) = source.read_at_most_while(1, |_| ReadInspectDecision::Complete(1)).await.unwrap();
        assert_eq!(view, b"x");
        assert!(buffer.is_empty());
    });
}

#[test]
fn gamma_survivor_fake_read_zero_and_preferred_size_and_memory_forwarders() {
    block_on(async {
        let content: Vec<u8> = (0..205).map(|index| u8::try_from(index).unwrap()).collect();
        let mut source = FakeRead::new(BytesView::from(content.clone()));
        assert_eq!(Memory::reserve(&source, 7).capacity(), 7);
        let mut buffer = source.reserve(9);
        buffer.put_slice(*b"pre");
        let (read, buffer) = source.read_at_most_into(0, buffer).await.unwrap();
        assert_eq!(read, 0);
        assert_eq!(buffer.peek(), b"pre");
        assert_eq!(buffer.capacity(), 9);
        let (read, mut buffer) = source.read_more_into(buffer).await.unwrap();
        assert_eq!(read, 100);
        assert_eq!(buffer.consume(3), b"pre");
        assert_eq!(buffer.consume_all(), &content[..100]);
        assert_eq!(source.read_any().await.unwrap().consume_all(), &content[100..200]);
        assert_eq!(source.read_any().await.unwrap().consume_all(), &content[200..]);
        assert!(source.read_any().await.unwrap().is_empty());
        assert_eq!(Memory::reserve(&Null::new(), 7).capacity(), 7);
        assert_eq!(Memory::reserve(&Pending::new(), 7).capacity(), 7);
        let mut null = Null::new();
        let mut buffer = null.reserve(9);
        buffer.put_slice(*b"kept");
        let (read, buffer) = Read::read_at_most_into(&mut null, 0, buffer).await.unwrap();
        assert_eq!((read, buffer.capacity()), (0, 9));
        assert_eq!(buffer.peek(), b"kept");
        let (read, buffer) = Read::read_at_most_into(&mut null, usize::MAX, buffer).await.unwrap();
        assert_eq!((read, buffer.capacity()), (0, 9));
        assert_eq!(buffer.peek(), b"kept");
        Write::write(&mut null, BytesView::from(b"discard".to_vec())).await.unwrap();
    });
}

#[test]
fn gamma_survivor_pending_read_and_write_stay_pending_without_runtime() {
    let mut source = Pending::new();
    let mut context = Context::from_waker(Waker::noop());
    for length in [0, 1, 7, usize::MAX] {
        let future = Read::read_at_most_into(&mut source, length, BytesBuf::new());
        let mut future = pin!(future);
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(future.as_mut().poll(&mut context).is_pending());
    }
    let future = Write::write(&mut source, BytesView::from(b"pending".to_vec()));
    let mut future = pin!(future);
    assert!(future.as_mut().poll(&mut context).is_pending());
    assert!(future.as_mut().poll(&mut context).is_pending());
}

#[cfg(feature = "futures-stream")]
#[derive(Debug, Default)]
struct GammaSurvivorBorrowedRead {
    released: bool,
}

#[cfg(feature = "futures-stream")]
impl Memory for GammaSurvivorBorrowedRead {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        TransparentMemory::new().reserve(min_bytes)
    }
}

#[cfg(feature = "futures-stream")]
impl HasMemory for GammaSurvivorBorrowedRead {
    fn memory(&self) -> impl MemoryShared {
        TransparentMemory::new()
    }
}

#[cfg(feature = "futures-stream")]
impl Read for GammaSurvivorBorrowedRead {
    type Error = std::convert::Infallible;

    async fn read_at_most_into(&mut self, _: usize, _: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        pending().await
    }

    async fn read_more_into(&mut self, _: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        pending().await
    }

    async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
        struct GammaSurvivorRelease<'a>(&'a mut bool);
        impl Drop for GammaSurvivorRelease<'_> {
            fn drop(&mut self) {
                *self.0 = true;
            }
        }
        let _release = GammaSurvivorRelease(&mut self.released);
        pending().await
    }
}

#[cfg(feature = "futures-stream")]
#[test]
fn gamma_survivor_stream_cancels_borrow_before_returning_inner() {
    use futures_core::Stream;

    let mut stream = GammaSurvivorBorrowedRead::default().into_futures_stream();
    let debug = format!("{stream:?}");
    assert!(debug.contains("inner: GammaSurvivorBorrowedRead { released: false }"));
    assert!(debug.contains("active_read.is_some(): false"));
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(stream.as_mut().poll_next(&mut context), Poll::Pending);
    assert!(format!("{stream:?}").contains("active_read.is_some(): true"));
    let source = stream.into_inner();
    assert!(source.released);
}
