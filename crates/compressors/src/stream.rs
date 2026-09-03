// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compression and decompression as a [`futures_core::Stream`].
//!
//! [`CompressionStream`] wraps a stream of byte sequences and yields converted chunks as they
//! become available, so a body of any size passes through in bounded memory. Both the source and
//! compression operation remain generic. Requires the `futures-stream` cargo feature.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use futures_core::Stream;
use pin_project_lite::pin_project;

use crate::core::{Compress, Compression, Decompress, Output};
use crate::error::{Error, Result};

/// Bounds the amount of immediately-ready work one `poll_next` performs.
const MAX_OPERATIONS_PER_POLL: usize = 64;

/// Drives a compression operation from a source stream.
///
/// The source is polled only when the operation has nothing left to give, so a slow consumer never
/// causes unbounded buffering.
///
/// `finished` latches once the stream has yielded its last item. Without it, a failing codec would
/// report the same error on every subsequent poll, and a caller that collects the stream would
/// accumulate errors until it ran out of memory.
/// Drives one poll of a compression stream, whichever direction it runs in.
///
/// Answering with data unconditionally produces a stream that never ends, so that mutant hangs
/// rather than failing and the harness records a timeout instead of a verdict.
#[cfg_attr(test, mutants::skip)]
fn poll_compression<S, C, E>(
    mut source: Pin<&mut S>,
    compression: &mut C,
    finished: &mut bool,
    cx: &mut Context<'_>,
) -> Poll<Option<Result<BytesView>>>
where
    S: Stream<Item = std::result::Result<BytesView, E>>,
    C: Compression + ?Sized,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    if *finished {
        return Poll::Ready(None);
    }

    for _ in 0..MAX_OPERATIONS_PER_POLL {
        match compression.pull() {
            Err(error) => {
                *finished = true;
                return Poll::Ready(Some(Err(error)));
            }
            Ok(Output::Data(data)) => return Poll::Ready(Some(Ok(data))),
            Ok(Output::Progress) => {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Ok(Output::Done) => {
                *finished = true;
                return Poll::Ready(None);
            }
            Ok(Output::NeedInput) => match source.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => compression.end_input(),
                Poll::Ready(Some(Ok(chunk))) => {
                    if let Err(error) = compression.push(chunk) {
                        *finished = true;
                        return Poll::Ready(Some(Err(error)));
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    *finished = true;
                    return Poll::Ready(Some(Err(Error::source(error))));
                }
            },
        }
    }

    cx.waker().wake_by_ref();
    Poll::Pending
}

pin_project! {
    /// Compresses or decompresses a stream of [`BytesView`] values.
    ///
    /// Construct it with [`CompressionStream::compress`] or [`CompressionStream::decompress`].
    /// Both the source and operation retain their concrete types; this adapter performs no boxing.
    ///
    /// The stream ends after its first error rather than reporting the same failure repeatedly.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytesbuf::BytesView;
    /// use bytesbuf::mem::GlobalPool;
    /// use compressors::{CompressionStream, Resources, gzip};
    /// use futures::StreamExt;
    /// use futures::stream;
    ///
    /// # futures::executor::block_on(async {
    /// let memory = GlobalPool::new();
    /// let source = stream::iter(vec![
    ///     Ok::<_, std::io::Error>(BytesView::copied_from_slice(b"first ", &memory)),
    ///     Ok(BytesView::copied_from_slice(b"second", &memory)),
    /// ]);
    ///
    /// let chunks: Vec<_> =
    ///     CompressionStream::compress(source, gzip::Compressor::new(&Resources::default())).collect().await;
    /// let gzip = BytesView::from_views(chunks.into_iter().map(|c| c.unwrap()));
    ///
    /// assert_eq!(gzip.range(0..2).to_vec(), vec![0x1f, 0x8b]);
    /// # });
    /// ```
    #[derive(Debug)]
    pub struct CompressionStream<S, C> {
        #[pin]
        source: S,
        compression: C,
        finished: bool,
    }
}

impl<S, C> CompressionStream<S, C>
where
    C: Compression<Mode = Compress>,
{
    /// Compresses `source` with `compression`.
    ///
    /// # Examples
    ///
    /// See [`CompressionStream`] for a complete example.
    #[must_use]
    pub fn compress(source: S, compression: C) -> Self {
        Self {
            source,
            compression,
            finished: false,
        }
    }
}

impl<S, C> CompressionStream<S, C>
where
    C: Compression<Mode = Decompress>,
{
    /// Decompresses `source` with `compression`.
    ///
    /// # Security
    ///
    /// This adapter adds no bounds of its own; it keeps whatever the supplied decompressor was
    /// built with, and hands every chunk straight back rather than accumulating. A decompressor
    /// built with its format's `new` therefore carries only that format's ratio bound. If the
    /// consumer buffers what this yields, build the decompressor with its `builder` and set
    /// [`with_max_output_len`][crate::DecompressorLimits::with_max_output_len] to what that consumer
    /// can afford.
    ///
    /// Output chunks are provisional until the stream ends, because a checksum or trailer can
    /// reject the compressed stream after earlier bytes have been returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytesbuf::BytesView;
    /// use bytesbuf::mem::GlobalPool;
    /// use compressors::{CompressionStream, Resources, gzip};
    /// use futures::{StreamExt, stream};
    ///
    /// # futures::executor::block_on(async {
    /// let memory = GlobalPool::new();
    /// let compressed = gzip::compress(
    ///     BytesView::copied_from_slice(b"payload", &memory),
    ///     &Resources::default(),
    /// )
    /// .unwrap();
    ///
    /// // Deliver the gzip stream one byte at a time, the worst case for a decompressor.
    /// let source = stream::iter(
    ///     (0..compressed.len())
    ///         .map(|i| Ok::<_, std::io::Error>(compressed.range(i..i + 1)))
    ///         .collect::<Vec<_>>(),
    /// );
    ///
    /// let chunks: Vec<_> =
    ///     CompressionStream::decompress(source, gzip::Decompressor::new(&Resources::default()))
    ///         .collect()
    ///         .await;
    /// let plain = BytesView::from_views(chunks.into_iter().map(|c| c.unwrap()));
    ///
    /// assert_eq!(plain.to_vec(), b"payload".to_vec());
    /// # });
    /// ```
    #[must_use]
    pub fn decompress(source: S, compression: C) -> Self {
        Self {
            source,
            compression,
            finished: false,
        }
    }
}

impl<S, C, E> Stream for CompressionStream<S, C>
where
    S: Stream<Item = std::result::Result<BytesView, E>>,
    C: Compression,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = Result<BytesView>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        poll_compression(this.source, this.compression, this.finished, cx)
    }
}

#[cfg(all(test, feature = "gzip"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::BytesBuf;
    use bytesbuf::mem::GlobalPool;
    use futures::executor::block_on;
    use futures::task::noop_waker;
    use futures::{StreamExt, stream};

    use super::*;
    use crate::core::ProgressCompression;
    use crate::{DecompressorLimits, Format, Level, Resources, gzip};

    fn view(bytes: &[u8]) -> BytesView {
        BytesView::copied_from_slice(bytes, &GlobalPool::new())
    }

    fn ok_stream(chunks: Vec<BytesView>) -> impl Stream<Item = std::result::Result<BytesView, std::io::Error>> {
        stream::iter(chunks.into_iter().map(Ok))
    }

    fn collect(stream: impl Stream<Item = Result<BytesView>>) -> Result<BytesView> {
        block_on(async {
            let chunks: Vec<_> = stream.collect().await;
            let mut collected = BytesBuf::new();
            for chunk in chunks {
                collected.put_bytes(chunk?);
            }
            Ok(collected.consume_all())
        })
    }

    #[test]
    fn round_trips_through_both_directions() {
        let payload = b"streaming round trip ".repeat(500);

        let source = ok_stream(payload.chunks(97).map(view).collect());
        let gzip =
            collect(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).expect("compression succeeds");

        let plain = collect(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .expect("decompression succeeds");

        assert_eq!(plain.to_vec(), payload);
    }

    /// Every format must support both directions.
    #[test]
    fn every_format_round_trips_through_the_stream() {
        let payload = b"every format ".repeat(300);
        let chunks = || ok_stream(payload.chunks(89).map(view).collect());

        // Every format reaches the stream through `Format`, so this needs no per-format arm and
        // cannot fall out of step when a format is added.
        for &format in Format::ALL {
            let compressor = crate::CompressorBuilder::new()
                .build_format(format, &Resources::default())
                .expect("the default settings are accepted");
            let compressed = collect(CompressionStream::compress(chunks(), compressor)).expect("compression succeeds");

            let decompressor = crate::DecompressorBuilder::new()
                .build_format(format, &Resources::default())
                .expect("the default settings are accepted");
            let plain = collect(CompressionStream::decompress(ok_stream(vec![compressed]), decompressor)).expect("decompression succeeds");

            assert_eq!(plain.to_vec(), payload, "{format:?} failed to round trip");
        }
    }

    #[test]
    fn compresses_an_empty_source() {
        let source = ok_stream(Vec::new());
        let gzip =
            collect(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).expect("compression succeeds");

        assert_eq!(gzip.range(0..2).to_vec(), vec![0x1f, 0x8b]);
    }

    #[test]
    fn decompresses_a_byte_at_a_time() {
        let compressed = crate::gzip::compress(view(b"one byte at a time"), &Resources::default()).expect("compression succeeds");
        let single_bytes = (0..compressed.len()).map(|i| compressed.range(i..=i)).collect();

        let plain = collect(CompressionStream::decompress(
            ok_stream(single_bytes),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .expect("decompression succeeds");

        assert_eq!(plain.to_vec(), b"one byte at a time".to_vec());
    }

    #[test]
    fn decompresses_members_delivered_as_separate_source_items() {
        let first = crate::gzip::compress(view(b"first"), &Resources::default()).expect("compression succeeds");
        let second = crate::gzip::compress(view(b"second"), &Resources::default()).expect("compression succeeds");

        let plain = collect(CompressionStream::decompress(
            ok_stream(vec![first, second]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .expect("both members decompress");

        assert_eq!(plain.to_vec(), b"firstsecond".to_vec());
    }

    #[test]
    fn reports_a_failing_source_as_a_source_error() {
        let failing = stream::iter(vec![Err(std::io::Error::other("transport died"))]);

        let error = collect(CompressionStream::compress(failing, gzip::Compressor::new(&Resources::default())))
            .expect_err("the source failure surfaces");

        assert!(error.is_source(), "got {error}");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("transport died".to_owned()),
            "the original failure should remain reachable"
        );
    }

    #[test]
    fn accepts_source_errors_convertible_to_a_boxed_error() {
        let failing = stream::iter(vec![Err("transport died".to_owned())]);

        let error = collect(CompressionStream::compress(failing, gzip::Compressor::new(&Resources::default())))
            .expect_err("the source failure surfaces");

        assert!(error.is_source(), "got {error}");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("transport died".to_owned())
        );
    }

    #[test]
    fn reports_a_push_rejection_as_an_error() {
        use crate::core::RejectsPush;

        let source = ok_stream(vec![view(b"chunk")]);
        let error = collect(CompressionStream::compress(source, RejectsPush)).expect_err("the push failure surfaces");

        assert!(error.is_invalid_state(), "got {error}");
    }

    #[test]
    fn rejects_push_fixture_end_input_is_a_no_op() {
        use crate::core::RejectsPush;

        let mut operation = RejectsPush;
        operation.end_input();
    }

    #[test]
    fn ends_after_the_first_error_instead_of_repeating_it() {
        // A stream that keeps yielding the same error is unbounded: a caller that collects it
        // accumulates errors until it runs out of memory.
        let source = ok_stream(vec![view(b"this is not gzip")]);
        let mut stream = Box::pin(CompressionStream::decompress(
            source,
            gzip::Decompressor::new(&Resources::default()),
        ));

        block_on(async {
            let first = stream.next().await.expect("an error is reported");
            assert!(first.expect_err("the data is invalid").is_corrupt_data());

            assert!(stream.next().await.is_none(), "the stream must end after an error");
            assert!(stream.next().await.is_none(), "and stay ended");
        });
    }

    #[test]
    fn stays_ended_after_completion() {
        let gzip = crate::gzip::compress(view(b"done"), &Resources::default()).expect("compression succeeds");
        let mut stream = Box::pin(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ));

        block_on(async {
            while stream.next().await.is_some() {}
            assert!(stream.next().await.is_none(), "a completed stream stays ended");
        });
    }

    #[test]
    fn reports_corrupt_input_from_decompression() {
        let source = ok_stream(vec![view(b"this is not gzip")]);

        let error = collect(CompressionStream::decompress(
            source,
            gzip::Decompressor::new(&Resources::default()),
        ))
        .expect_err("bad data is rejected");

        assert!(error.is_corrupt_data(), "got {error}");
    }

    #[test]
    fn honours_a_pre_configured_decompressor() {
        let gzip = crate::gzip::compress(view(&vec![0_u8; 4 * 1024 * 1024]), &Resources::default()).expect("compression succeeds");

        let decompressor = gzip::Decompressor::builder()
            .limits(DecompressorLimits::new().with_max_output_len(1024))
            .build(&Resources::default());

        let error = collect(CompressionStream::decompress(ok_stream(vec![gzip]), decompressor)).expect_err("the cap fires");

        assert!(error.is_limit_exceeded(), "got {error}");
    }

    #[test]
    fn honours_a_pre_configured_compressor() {
        let payload = b"the quick brown fox ".repeat(400);

        let compressor = gzip::Compressor::builder().level(Level::HIGH).build(&Resources::default());
        let gzip = collect(CompressionStream::compress(ok_stream(vec![view(&payload)]), compressor)).expect("compression succeeds");

        let plain = collect(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .expect("decompression succeeds");

        assert_eq!(plain.to_vec(), payload);
    }

    #[test]
    fn tolerates_empty_chunks_from_the_source() {
        let source = ok_stream(vec![BytesView::new(), view(b"data"), BytesView::new()]);

        let gzip =
            collect(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).expect("compression succeeds");
        let plain = collect(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .expect("decompression succeeds");

        assert_eq!(plain.to_vec(), b"data".to_vec());
    }

    #[test]
    fn waits_for_a_pending_source() {
        // Exercises the `Poll::Pending` arm: the source stalls once before ending.
        let mut stalled = false;
        let source = stream::poll_fn(move |cx| -> Poll<Option<std::result::Result<BytesView, std::io::Error>>> {
            if stalled {
                return Poll::Ready(None);
            }

            stalled = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        });

        let gzip =
            collect(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).expect("compression succeeds");

        assert_eq!(gzip.range(0..2).to_vec(), vec![0x1f, 0x8b]);
    }

    #[test]
    fn bounds_immediately_ready_empty_source_items_per_poll() {
        struct ReadyEmpty {
            polls: Arc<AtomicUsize>,
        }

        impl Stream for ReadyEmpty {
            type Item = std::result::Result<BytesView, std::io::Error>;

            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                self.polls.fetch_add(1, Ordering::Relaxed);
                Poll::Ready(Some(Ok(BytesView::new())))
            }
        }

        let polls = Arc::new(AtomicUsize::new(0));
        let source = ReadyEmpty { polls: Arc::clone(&polls) };
        let mut stream = Box::pin(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default())));
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut yielded = false;
        for _ in 0..4 {
            if stream.as_mut().poll_next(&mut cx).is_pending() {
                yielded = true;
                break;
            }
        }

        assert!(yielded, "the adapter must yield after a bounded amount of ready work");
        assert_eq!(polls.load(Ordering::Relaxed), MAX_OPERATIONS_PER_POLL);
    }

    #[test]
    fn progress_yields_after_one_codec_pull() {
        let pulls = Arc::new(AtomicUsize::new(0));
        let source = stream::pending::<std::result::Result<BytesView, std::io::Error>>();
        let mut stream = Box::pin(CompressionStream::compress(source, ProgressCompression::new(Arc::clone(&pulls))));
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(stream.as_mut().poll_next(&mut cx).is_pending());
        assert_eq!(pulls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_progress_fixture_accepts_push_and_end_input_directly() {
        use crate::core::Compression;

        let mut operation = ProgressCompression::new(Arc::new(AtomicUsize::new(0)));
        operation.push(view(b"ignored")).expect("the fixture always accepts pushed input");
        operation.end_input();
    }

    #[test]
    fn streams_are_send_so_they_can_cross_task_boundaries() {
        // `!Send` is infectious: a stream that cannot move between tasks is unusable in most async
        // runtimes.
        fn assert_send<T: Send>(_: &T) {}
        assert_send(&CompressionStream::compress(
            ok_stream(Vec::new()),
            gzip::Compressor::new(&Resources::default()),
        ));
        assert_send(&CompressionStream::decompress(
            ok_stream(Vec::new()),
            gzip::Decompressor::new(&Resources::default()),
        ));
    }

    #[test]
    fn debug_is_available_for_diagnostics() {
        let empty = stream::iter(Vec::<std::result::Result<BytesView, std::io::Error>>::new());
        let stream = CompressionStream::compress(empty, gzip::Compressor::new(&Resources::default()));

        assert!(format!("{stream:?}").contains("CompressionStream"));
    }
}
