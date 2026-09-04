// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compression and decompression as a [`futures_core::Stream`].
//!
//! [`CompressionStream`] wraps a stream of byte sequences and yields converted chunks as they
//! become available, so a body of any size passes through in bounded memory. Both the source and
//! compression engine remain generic. Requires the `futures-stream` cargo feature.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use futures_core::Stream;
use pin_project_lite::pin_project;

use crate::core::{Compress, Compression, Decompress, Destination, Output};
use crate::error::{Error, Result};

/// Bounds the amount of immediately-ready work one `poll_next` performs.
///
/// A stream whose source is always ready would otherwise let one poll run until the data ends,
/// starving the executor's other tasks. The rule is to yield often enough to stay fair while
/// amortizing the wake machinery over more than a single chunk; the value is a conservative
/// starting point, not a measured optimum, and matches the engine's per-`pull` step budget so the
/// two layers bound work on the same scale.
const MAX_OPERATIONS_PER_POLL: usize = 64;

/// Drives one poll of a compression stream, whichever direction it runs in.
///
/// The source is polled only when the engine has nothing left to give, so a slow consumer never
/// causes unbounded buffering.
///
/// `finished` latches once the stream has yielded its last item. Without it, a failing engine would
/// report the same error on every subsequent poll, and a caller that collects the stream would
/// accumulate errors until it ran out of memory.
fn poll_compression<S, C, E>(
    mut source: Pin<&mut S>,
    compression: &mut C,
    finished: &mut bool,
    cx: &mut Context<'_>,
) -> Poll<Option<Result<BytesView>>>
where
    S: Stream<Item = std::result::Result<BytesView, E>>,
    C: Compression,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    if *finished {
        return Poll::Ready(None);
    }

    // Latches when the source has run dry and `end_input` has been signalled. A conforming codec
    // answers the next `pull` with output or `Done`, never with another request for input, so a
    // second `NeedInput` means the codec is not honouring the contract. Without this the loop would
    // poll the exhausted source again, re-signal end of input, and keep waking itself: bounded per
    // poll, but a livelock across them. `process` rejects the same sequence outright.
    let mut input_ended = false;

    for _ in 0..MAX_OPERATIONS_PER_POLL {
        match compression.pull(Destination::Stream) {
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
            Ok(Output::NeedInput) if input_ended => {
                *finished = true;
                return Poll::Ready(Some(Err(Error::invalid_state("the operation requested input after end of input"))));
            }
            Ok(Output::NeedInput) => match source.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    compression.end_input();
                    input_ended = true;
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    if let Err(error) = compression.push(chunk) {
                        *finished = true;
                        return Poll::Ready(Some(Err(error)));
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    *finished = true;
                    return Poll::Ready(Some(Err(Error::other("the underlying stream failed", error))));
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
    /// Both the source and the engine retain their concrete types; this adapter performs no boxing.
    ///
    /// The source yields `Result<BytesView, E>` rather than bare views, for any `E` that converts
    /// into a boxed `std::error::Error + Send + Sync`. A source failure ends the stream, reported as
    /// an [`Error`][crate::Error] for which [`is_source`][crate::Error::is_source] is true and whose
    /// [`source`][std::error::Error::source] is the original. The constructors accept any `S`, so a
    /// source of plain views compiles and only fails to satisfy [`Stream`] when it is polled.
    ///
    /// The stream ends after its first error rather than reporting the same failure repeatedly.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "gzip")]
    /// # {
    /// use bytesbuf::BytesView;
    /// use bytesbuf::mem::GlobalPool;
    /// use compressors::{CompressionStream, Resources, gzip};
    /// use futures::{StreamExt, stream};
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
    /// # }
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
    /// [`max_output_len`][crate::DecompressorLimits::max_output_len] to what that consumer
    /// can afford.
    ///
    /// An output cap is not the whole story for a format that joins concatenated streams -- gzip
    /// and zstd do so by default. Cumulative output is what
    /// [`max_output_len`][crate::DecompressorLimits::max_output_len] bounds, so a long run
    /// of small or empty members can keep decoding without ever reaching it. Bound the member count
    /// as well with [`max_streams`][crate::DecompressorLimits::max_streams], or turn
    /// joining off with [`multi_stream(false)`][crate::DecompressorBuilder::multi_stream].
    ///
    /// Output chunks are provisional until the stream ends, because a checksum or trailer can
    /// reject the compressed stream after earlier bytes have been returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "gzip")]
    /// # {
    /// use std::num::NonZeroU64;
    ///
    /// use bytesbuf::BytesView;
    /// use compressors::{CompressionStream, DecompressorLimits, Resources, gzip};
    /// use futures::{StreamExt, stream};
    ///
    /// # futures::executor::block_on(async {
    /// let compressed = gzip::compress(b"payload", &Resources::default()).unwrap();
    ///
    /// // Deliver the gzip stream one byte at a time, the worst case for a decompressor.
    /// let source = stream::iter(
    ///     (0..compressed.len())
    ///         .map(|i| Ok::<_, std::io::Error>(compressed.range(i..i + 1)))
    ///         .collect::<Vec<_>>(),
    /// );
    ///
    /// // This consumer collects every chunk, so it caps the total rather than relying on the
    /// // adapter's bounded working set.
    /// let decompressor = gzip::Decompressor::builder()
    ///     .limits(DecompressorLimits::new().max_output_len(NonZeroU64::new(1 << 20).unwrap()))
    ///     .build(&Resources::default());
    ///
    /// let chunks: Vec<_> = CompressionStream::decompress(source, decompressor)
    ///     .collect()
    ///     .await;
    /// let plain = BytesView::from_views(chunks.into_iter().map(|c| c.unwrap()));
    ///
    /// assert_eq!(plain.to_vec(), b"payload".to_vec());
    /// # });
    /// # }
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::BytesBuf;
    use futures::executor::block_on;
    use futures::task::{ArcWake, noop_waker};
    use futures::{StreamExt, stream};

    use super::*;
    use crate::format::Format;
    use crate::testing::{ProgressCompression, view};
    use crate::{DecompressorLimits, Level, Resources, gzip};

    fn ok_stream(chunks: Vec<BytesView>) -> impl Stream<Item = std::result::Result<BytesView, std::io::Error>> {
        stream::iter(chunks.into_iter().map(Ok))
    }

    /// Caps the polls any test here may need.
    ///
    /// A conforming stream terminates, so exceeding this means the code under test is spinning or
    /// emitting endlessly. Draining without a bound would hang or exhaust memory instead, and a
    /// hanging test reports nothing at all -- which is also what stops mutation testing from
    /// reaching a verdict rather than a timeout.
    const MAX_POLLS: usize = 10_000;

    /// A waker that records whether it was asked to wake anything.
    #[derive(Debug, Default)]
    struct CountingWaker(AtomicUsize);

    impl ArcWake for CountingWaker {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Drives `stream` to completion and returns everything it yielded.
    ///
    /// Polls directly rather than through an executor, so the number of polls is bounded and a
    /// `Pending` that arranged no wake is a failure rather than a hang.
    fn drain(stream: impl Stream<Item = Result<BytesView>>) -> Result<BytesView> {
        let scheduled = Arc::new(CountingWaker::default());
        let waker = futures::task::waker(Arc::clone(&scheduled));
        let mut context = Context::from_waker(&waker);
        let mut stream = pin!(stream);
        let mut collected = BytesBuf::new();

        for poll in 0..MAX_POLLS {
            match stream.as_mut().poll_next(&mut context) {
                Poll::Ready(Some(item)) => collected.put_bytes(item?),
                Poll::Ready(None) => return Ok(collected.consume_all()),
                Poll::Pending => assert!(
                    scheduled.0.load(Ordering::Relaxed) > 0,
                    "the stream returned Pending on poll {poll} without arranging a wake"
                ),
            }
        }

        panic!("the stream did not finish within {MAX_POLLS} polls");
    }

    #[test]
    fn round_trips_through_both_directions() {
        let payload = b"streaming round trip ".repeat(500);

        let source = ok_stream(payload.chunks(97).map(view).collect());
        let gzip = drain(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).unwrap();

        let plain = drain(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .unwrap();

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
            let compressor = crate::CompressorBuilder::new().build_format(format, &Resources::default()).unwrap();
            let compressed = drain(CompressionStream::compress(chunks(), compressor)).unwrap();

            let decompressor = crate::DecompressorBuilder::new()
                .build_format(format, &Resources::default())
                .unwrap();
            let plain = drain(CompressionStream::decompress(ok_stream(vec![compressed]), decompressor)).unwrap();

            assert_eq!(plain.to_vec(), payload, "{format:?} failed to round trip");
        }
    }

    #[test]
    fn compresses_an_empty_source() {
        let source = ok_stream(Vec::new());
        let gzip = drain(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).unwrap();

        assert_eq!(gzip.range(0..2).to_vec(), vec![0x1f, 0x8b]);
    }

    #[test]
    fn decompresses_a_byte_at_a_time() {
        let compressed = crate::gzip::compress(view(b"one byte at a time"), &Resources::default()).unwrap();
        let single_bytes = (0..compressed.len()).map(|i| compressed.range(i..=i)).collect();

        let plain = drain(CompressionStream::decompress(
            ok_stream(single_bytes),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .unwrap();

        assert_eq!(plain.to_vec(), b"one byte at a time".to_vec());
    }

    #[test]
    fn decompresses_members_delivered_as_separate_source_items() {
        let first = crate::gzip::compress(view(b"first"), &Resources::default()).unwrap();
        let second = crate::gzip::compress(view(b"second"), &Resources::default()).unwrap();

        let plain = drain(CompressionStream::decompress(
            ok_stream(vec![first, second]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .unwrap();

        assert_eq!(plain.to_vec(), b"firstsecond".to_vec());
    }

    #[test]
    fn reports_a_failing_source_as_a_source_error() {
        let failing = stream::iter(vec![Err(std::io::Error::other("transport died"))]);

        let error = drain(CompressionStream::compress(failing, gzip::Compressor::new(&Resources::default()))).unwrap_err();

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

        let error = drain(CompressionStream::compress(failing, gzip::Compressor::new(&Resources::default()))).unwrap_err();

        assert!(error.is_source(), "got {error}");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("transport died".to_owned())
        );
    }

    #[test]
    fn reports_a_push_rejection_as_an_error() {
        use crate::testing::RejectsPush;

        let source = ok_stream(vec![view(b"chunk")]);
        let error = drain(CompressionStream::compress(source, RejectsPush)).unwrap_err();

        assert!(error.is_invalid_state(), "got {error}");
    }

    #[test]
    fn reports_a_request_for_input_after_end_of_input_as_an_error() {
        // The fixture asks for input forever. Once the source is exhausted there is none left to
        // give, so the adapter has to end the stream rather than keep waking itself to ask again.
        use crate::testing::RejectsPush;

        let source = ok_stream(Vec::new());
        let error = drain(CompressionStream::compress(source, RejectsPush)).unwrap_err();

        assert!(error.is_invalid_state(), "got {error}");
    }

    #[test]
    fn rejects_push_fixture_end_input_is_a_no_op() {
        use crate::core::CompressionInternal as _;
        use crate::testing::RejectsPush;

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
            let first = stream.next().await.unwrap();
            assert!(first.unwrap_err().is_corrupt_data());

            assert!(stream.next().await.is_none(), "the stream must end after an error");
            assert!(stream.next().await.is_none(), "and stay ended");
        });
    }

    #[test]
    fn stays_ended_after_completion() {
        let gzip = crate::gzip::compress(view(b"done"), &Resources::default()).unwrap();
        let mut stream = pin!(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ));

        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let mut ended = false;
        for _ in 0..MAX_POLLS {
            if matches!(stream.as_mut().poll_next(&mut context), Poll::Ready(None)) {
                ended = true;
                break;
            }
        }
        assert!(ended, "the stream did not end within {MAX_POLLS} polls");

        assert!(
            matches!(stream.as_mut().poll_next(&mut context), Poll::Ready(None)),
            "a completed stream stays ended"
        );
    }

    #[test]
    fn reports_corrupt_input_from_decompression() {
        let source = ok_stream(vec![view(b"this is not gzip")]);

        let error = drain(CompressionStream::decompress(
            source,
            gzip::Decompressor::new(&Resources::default()),
        ))
        .unwrap_err();

        assert!(error.is_corrupt_data(), "got {error}");
    }

    #[test]
    fn honours_a_pre_configured_decompressor() {
        let gzip = crate::gzip::compress(view(&vec![0_u8; 4 * 1024 * 1024]), &Resources::default()).unwrap();

        let decompressor = gzip::Decompressor::builder()
            .limits(DecompressorLimits::new().max_output_len(NonZeroU64::new(1024).unwrap()))
            .build(&Resources::default());

        let error = drain(CompressionStream::decompress(ok_stream(vec![gzip]), decompressor)).unwrap_err();

        assert!(error.is_limit_exceeded(), "got {error}");
    }

    #[test]
    fn honours_a_pre_configured_compressor() {
        let payload = b"the quick brown fox ".repeat(400);

        let compressor = gzip::Compressor::builder().level(Level::HIGH).build(&Resources::default());
        let gzip = drain(CompressionStream::compress(ok_stream(vec![view(&payload)]), compressor)).unwrap();

        let plain = drain(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .unwrap();

        assert_eq!(plain.to_vec(), payload);
    }

    #[test]
    fn tolerates_empty_chunks_from_the_source() {
        let source = ok_stream(vec![BytesView::new(), view(b"data"), BytesView::new()]);

        let gzip = drain(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).unwrap();
        let plain = drain(CompressionStream::decompress(
            ok_stream(vec![gzip]),
            gzip::Decompressor::new(&Resources::default()),
        ))
        .unwrap();

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

        let gzip = drain(CompressionStream::compress(source, gzip::Compressor::new(&Resources::default()))).unwrap();

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
        use crate::core::CompressionInternal as _;

        let mut operation = ProgressCompression::new(Arc::new(AtomicUsize::new(0)));
        operation.push(view(b"ignored")).unwrap();
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
