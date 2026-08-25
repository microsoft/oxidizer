// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::ptr::NonNull;
use std::task::{Context, Poll};
use std::{fmt, mem};

use bytesbuf::BytesBuf;
use bytesbuf::mem::GlobalPool;
use fetch::HttpError;
use http::HeaderMap;
use http_body::{Body, Frame, SizeHint};

use crate::bindings::{Bindings as _, BindingsFacade};
use crate::context::{CompletionResult, OperationBuffer, OperationKind};
use crate::error::{callback_protocol_error, invalid_response, query_error};
use crate::operation::RequestGuard;
use crate::query::query_raw_trailers;
use crate::response_headers::parse_response_trailers;

// Bounds every read: it caps the reservation when WinHTTP reports a large
// availability, and supplies the size when WinHTTP reports zero available bytes
// yet can still complete a useful read. Large reads amortize the WinHTTP call
// and its worker-thread handoff over more bytes, and this is the default I/O
// size used across related projects when no better figure is known. It is an
// upper bound on the reservation only; what a single call submits is separately
// bounded by the buffer's first contiguous unfilled region, so this value takes
// no dependency on how `GlobalPool` sizes the blocks it hands out. Revisit it if
// the measured throughput-to-memory trade-off changes.
const PREFERRED_READ_SIZE: usize = 256 * 1024;

#[derive(Debug)]
/// Pulls one response body lazily from an ongoing WinHTTP request.
///
/// Each caller-driven read first queries available data and then lends one
/// contiguous writable region from a pooled `BytesBuf` to WinHTTP. Capacity is
/// reserved only after that query and only when the buffer cannot already serve
/// the read, so a peer that trickles bytes cannot make the transport rent pool
/// memory per byte delivered. A zero availability figure carries no size
/// information; the read is then speculative and reserves
/// `PREFERRED_READ_SIZE`, however few bytes come back. The retained
/// [`RequestGuard`] keeps callback state and handles alive, so dropping the
/// reader cancels the request without reading ahead. A zero-length
/// `READ_COMPLETE`, rather than a zero availability query, is authoritative
/// end-of-stream; trailers are queried only at that point.
///
/// EOF, read failure, cancellation, and drop all release the guard so WinHTTP
/// can deliver final closure and return the request context to its pool.
pub(crate) struct WinHttpBodyReader {
    guard: Option<RequestGuard>,
    bindings: BindingsFacade,
    memory: GlobalPool,
    trailers: Option<HeaderMap>,
    eof: bool,
}

impl WinHttpBodyReader {
    pub(crate) const fn new(guard: RequestGuard, bindings: BindingsFacade, memory: GlobalPool) -> Self {
        Self {
            guard: Some(guard),
            bindings,
            memory,
            trailers: None,
            eof: false,
        }
    }

    fn take_trailers(&mut self) -> Option<HeaderMap> {
        self.trailers.take()
    }

    /// Reads once, appending to `into`, and returns the byte count with the
    /// buffer that now owns those bytes.
    ///
    /// A zero count is authoritative end-of-stream. Trailers, when the response
    /// carries them, become available from [`take_trailers`](Self::take_trailers)
    /// at that point. Reads after end-of-stream report zero without reaching
    /// the transport.
    pub(crate) async fn read_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), HttpError> {
        if self.eof {
            return Ok((0, into));
        }

        let result = self.read_once(into).await;
        if result.is_err() {
            drop(self.guard.take());
        }
        result
    }

    async fn read_once(&mut self, mut into: BytesBuf) -> Result<(usize, BytesBuf), HttpError> {
        let available = {
            let bindings = self.bindings.clone();
            let query = self
                .guard_mut()?
                .submit(OperationKind::DataAvailable, OperationBuffer::none(), move |request, _context| {
                    // SAFETY: the reader owns a RequestGuard, which exists only
                    // where ContextInstallation::install accepted the fully
                    // initialized context on a request opened under the
                    // session's WINHTTP_FLAG_ASYNC handle, whose status callback
                    // was registered with the full notification mask before any
                    // request handle existed. submit() armed the slot for
                    // DataAvailable and moved the live request handle into the
                    // returned future, so the handle stays open for this call,
                    // the armed kind matches it, and the emptied guard admits no
                    // second operation while this one is outstanding. The query
                    // lends no buffer. No exclusive borrow of the context is
                    // outstanding across this call: the closure captures only a
                    // cloned facade, so a completion delivered inline reenters
                    // the callback on this thread with nothing but shared access
                    // in flight.
                    unsafe { bindings.query_data_available(request) }
                });
            let completion = query
                .await
                .map_err(|_disconnected| callback_protocol_error("the data-available completion channel disconnected"))?;
            data_available_completion(completion)?
        };

        let desired = if available == 0 {
            PREFERRED_READ_SIZE
        } else {
            usize::try_from(available).expect("a u32 always fits usize on supported Windows targets")
        }
        .min(PREFERRED_READ_SIZE);
        // A no-op whenever earlier reads left enough spare capacity behind, so
        // consecutive small reads are served from the buffer rather than from
        // the pool.
        into.reserve(desired, &self.memory);

        let (buffer, address, capacity) = {
            let region = into.first_unfilled_slice();
            let capacity = next_read_capacity(region.len(), desired);
            debug_assert!(capacity != 0, "a positive reservation must expose a writable region");
            let buffer = NonNull::new(region.as_mut_ptr().cast::<u8>()).expect("a positive writable region has a non-null pointer");
            (buffer, buffer.as_ptr().addr(), capacity)
        };

        let bindings = self.bindings.clone();
        let read = self.guard_mut()?.submit(
            OperationKind::Read,
            OperationBuffer::read(into, address, capacity),
            move |request, _context| {
                // SAFETY: the guard establishes the asynchronous session, the
                // registered status callback and the installed context exactly
                // as for the availability query above. submit() armed the slot
                // for Read and moved the live request handle into the returned
                // future, so the handle stays open for this call, the armed kind
                // matches it, and the emptied guard admits no second operation
                // while this one is outstanding. `buffer` addresses the start of
                // the writable region obtained above, and `capacity` is bounded
                // by that region's length, so the span is writable for
                // `capacity` bytes. The BytesBuf owning that region moves into
                // the operation buffer in this same call, so the request task
                // can neither read nor free the span until a completion, a
                // request error, or HANDLE_CLOSING hands the buffer back; moving
                // the BytesBuf value does not move the pooled block the region
                // lives in. A
                // synchronous failure reclaims the buffer through submit()'s
                // claim of the slot, which read_data's contract permits by
                // starting no read and keeping no reference when it reports
                // failure. No exclusive borrow of the context is outstanding
                // across this call: the closure captures only a cloned facade
                // and the span it lends, so a completion delivered inline
                // reenters the callback on this thread with nothing but shared
                // access in flight.
                unsafe { bindings.read_data(request, buffer, capacity) }
            },
        );
        let completion = read
            .await
            .map_err(|_disconnected| callback_protocol_error("the read completion channel disconnected"))?;
        let (read, mut into) = read_completion(completion)?;

        if read == 0 {
            let trailers = query_raw_trailers(&self.bindings, self.guard_ref()?.raw())
                .map_err(query_error)?
                .map(|raw| parse_response_trailers(&raw).map_err(invalid_response))
                .transpose()?;
            self.trailers = trailers;
            self.eof = true;
            drop(self.guard.take());
        } else {
            // SAFETY: advance() requires `read` initialized bytes at the start
            // of the current first unfilled slice. Nothing touched this buffer
            // between the submission and this completion, so that slice still
            // begins at the address lent to WinHTTP, and the callback accepted
            // the completion only after checking that WinHTTP reported that same
            // address and a length within the lent capacity, which is itself
            // bounded by the slice length. WinHttpReadData initializes exactly
            // that many bytes at the start of the buffer it was given.
            unsafe {
                into.advance(read);
            }
        }

        Ok((read, into))
    }

    fn guard_ref(&self) -> Result<&RequestGuard, HttpError> {
        self.guard
            .as_ref()
            .ok_or_else(|| callback_protocol_error("the response body request handle is already closed"))
    }

    fn guard_mut(&mut self) -> Result<&mut RequestGuard, HttpError> {
        self.guard
            .as_mut()
            .ok_or_else(|| callback_protocol_error("the response body request handle is already closed"))
    }
}

/// Preserves WinHTTP data and trailers as `http_body` frames.
///
/// The adapter detaches only the filled prefix of each read into the frame and
/// keeps the buffer, so consecutive reads refill capacity that the emitted
/// frames already hold. That is what bounds a peer that trickles bytes: the
/// pooled block a frame was cut from stays rented for as long as the consumer
/// holds the frame, so handing every frame its own buffer would let a slow
/// drip of one-byte frames rent one block each. Retaining the buffer instead
/// caps retained memory at the bytes actually delivered.
///
/// It yields data lazily, emits at most one final trailer frame, and disposes
/// of the reader after completion or error. The surrounding `HttpBodyBuilder`
/// remains responsible for applying the configured body idle timeout.
pub(crate) struct WinHttpResponseBody {
    state: BodyState,
}

/// Tracks which half of the read cycle the body is in.
///
/// `read_into` borrows the reader for the duration of one read, but
/// `poll_frame` must be able to return between polls of that read. The reader
/// therefore moves into the read future and comes back out of it, which keeps
/// the future free of borrows and lets it be boxed without self-reference.
#[expect(
    clippy::large_enum_variant,
    reason = "retaining the buffer inline is the point of this adapter; boxing it would restore the per-read allocation"
)]
enum BodyState {
    /// No read is outstanding. Holds the reader and whatever capacity the
    /// previous read left unused.
    Ready { reader: WinHttpBodyReader, buffer: BytesBuf },
    /// A read owns the reader and the buffer until it completes.
    Reading(BodyRead),
    /// End of stream, an error, or the trailer frame has been delivered.
    Done,
}

type BodyRead = Pin<Box<dyn Future<Output = ReadOutcome> + Send>>;

/// Returns the reader and buffer that [`read_step`] borrowed, with the outcome.
struct ReadOutcome {
    reader: WinHttpBodyReader,
    buffer: BytesBuf,
    result: Result<usize, HttpError>,
}

/// Performs one read, taking and returning ownership of the reader and buffer.
async fn read_step(mut reader: WinHttpBodyReader, buffer: BytesBuf) -> ReadOutcome {
    match reader.read_into(buffer).await {
        Ok((read, buffer)) => ReadOutcome {
            reader,
            buffer,
            result: Ok(read),
        },
        // A failed read consumes the buffer and ends the stream, so no capacity
        // is carried forward.
        Err(error) => ReadOutcome {
            reader,
            buffer: BytesBuf::new(),
            result: Err(error),
        },
    }
}

impl WinHttpResponseBody {
    pub(crate) fn new(reader: WinHttpBodyReader) -> Self {
        Self {
            state: BodyState::Ready {
                reader,
                buffer: BytesBuf::new(),
            },
        }
    }
}

impl fmt::Debug for WinHttpResponseBody {
    #[cfg_attr(coverage_nightly, coverage(off))] // We have no API contract here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state {
            BodyState::Ready { .. } => "ready",
            BodyState::Reading(_) => "reading",
            BodyState::Done => "done",
        };

        f.debug_struct("WinHttpResponseBody").field("state", &state).finish()
    }
}

impl Body for WinHttpResponseBody {
    type Data = bytesbuf::BytesView;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // Every arm either restores a live state or leaves `Done` in place, so
        // taking the state by value here cannot strand the body in `Done` while
        // work remains.
        match mem::replace(&mut self.state, BodyState::Done) {
            BodyState::Ready { reader, buffer } => {
                let mut read = Box::pin(read_step(reader, buffer));
                let outcome = match read.as_mut().poll(cx) {
                    Poll::Pending => {
                        self.state = BodyState::Reading(read);
                        return Poll::Pending;
                    }
                    Poll::Ready(outcome) => outcome,
                };

                self.deliver(outcome)
            }
            BodyState::Reading(mut read) => match read.as_mut().poll(cx) {
                Poll::Pending => {
                    self.state = BodyState::Reading(read);
                    Poll::Pending
                }
                Poll::Ready(outcome) => self.deliver(outcome),
            },
            BodyState::Done => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.state, BodyState::Done)
    }

    #[cfg_attr(test, mutants::skip)] // Intentionally unbounded; WinHTTP does not expose a remaining length.
    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

impl WinHttpResponseBody {
    /// Turns a completed read into the next frame, leaving `self.state` set.
    ///
    /// The caller has already replaced the state with `Done`, which is the
    /// correct final state for both an error and end of stream.
    fn deliver(mut self: Pin<&mut Self>, outcome: ReadOutcome) -> Poll<Option<Result<Frame<bytesbuf::BytesView>, HttpError>>> {
        let ReadOutcome {
            mut reader,
            mut buffer,
            result,
        } = outcome;

        let read = match result {
            Ok(read) => read,
            Err(error) => return Poll::Ready(Some(Err(error))),
        };

        if read == 0 {
            // The reader queries trailers at the authoritative zero-length
            // completion, so they are available as soon as it reports EOF.
            return Poll::Ready(reader.take_trailers().map(|trailers| Ok(Frame::trailers(trailers))));
        }

        // Detaching the filled prefix leaves the buffer holding the rest of the
        // block the frame already pins, so the next read refills that capacity
        // instead of renting a second block.
        let data = buffer.consume_all();
        // A positive read count with no bytes cannot arise from a completion
        // that passed `decode_read` (the length is bounded by the lent region).
        // The check stops a mutant that fabricates a nonzero count without a
        // buffer from spinning the body forever (AGENTS.md, "Code must not hang
        // even under mutation testing").
        if data.is_empty() {
            #[cfg_attr(coverage_nightly, coverage(off))] // Mutant/hang guard; unreachable under decode_read.
            {
                return Poll::Ready(Some(Err(callback_protocol_error(
                    "WinHTTP reported a nonempty read without returning any bytes",
                ))));
            }
        }
        self.state = BodyState::Ready { reader, buffer };

        Poll::Ready(Some(Ok(Frame::data(data))))
    }
}

fn data_available_completion(completion: CompletionResult) -> Result<u32, HttpError> {
    match completion {
        CompletionResult::DataAvailable(available) => Ok(available),
        CompletionResult::Error { error, .. } => Err(error.into_http_error()),
        CompletionResult::InvalidStatusInfo { status, len, .. } => Err(callback_protocol_error(format!(
            "WinHTTP returned invalid status information for callback 0x{status:08x} with {len} bytes"
        ))),
        unexpected => Err(callback_protocol_error(format!(
            "WinHTTP returned an unexpected completion for QueryDataAvailable: {unexpected:?}"
        ))),
    }
}

fn read_completion(completion: CompletionResult) -> Result<(usize, BytesBuf), HttpError> {
    match completion {
        CompletionResult::ReadComplete { buffer, len } => Ok((
            usize::try_from(len).expect("a u32 always fits usize on supported Windows targets"),
            buffer,
        )),
        CompletionResult::Error { error, .. } => Err(error.into_http_error()),
        CompletionResult::InvalidStatusInfo { status, len, .. } => Err(callback_protocol_error(format!(
            "WinHTTP returned invalid status information for callback 0x{status:08x} with {len} bytes"
        ))),
        unexpected => Err(callback_protocol_error(format!(
            "WinHTTP returned an unexpected completion for ReadData: {unexpected:?}"
        ))),
    }
}

fn next_read_capacity(region_len: usize, desired: usize) -> u32 {
    u32::try_from(region_len.min(desired).min(u32::MAX as usize)).expect("the read capacity is bounded by u32::MAX")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    use std::future::poll_fn;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::pin::Pin;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Waker};

    use bytesbuf::BytesBuf;
    use bytesbuf::mem::GlobalPool;
    use http::HeaderValue;
    use http_body::Body as _;
    use ohno::Labeled as _;
    use plurality::Pool;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::Networking::WinHttp::{
        ERROR_WINHTTP_HEADER_NOT_FOUND, WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
    };

    use super::{
        PREFERRED_READ_SIZE, WinHttpBodyReader, WinHttpResponseBody, data_available_completion, next_read_capacity, read_completion,
    };
    use crate::bindings::{BindingsFacade, MockBindings, WINHTTP_OPTION_CONTEXT_VALUE};
    use crate::context::{CompletionResult, RequestContext};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RequestHandle, SessionHandle};
    use crate::mocks::{closing, complete, complete_request_error, context_pointer, drive, installed_context, raw_handle, status_info_len};
    use crate::operation::{ContextInstallation, ContextPool};
    use crate::session::WinHttpSession;

    const SESSION: usize = 101;
    const CONNECT: usize = 102;
    const REQUEST: usize = 103;

    assert_impl_all!(WinHttpBodyReader: Send, std::fmt::Debug);
    assert_impl_all!(WinHttpResponseBody: Send, std::fmt::Debug, http_body::Body<Data = bytesbuf::BytesView, Error = fetch::HttpError>);
    // Both body types retain a guard pointing at the callback context's UnsafeCell state.
    assert_not_impl_any!(WinHttpBodyReader: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(WinHttpResponseBody: UnwindSafe, RefUnwindSafe);

    #[test]
    fn read_capacity_obeys_empty_caller_region_and_dword_bounds() {
        assert_eq!(next_read_capacity(0, usize::MAX), 0);
        assert_eq!(next_read_capacity(10, 4), 4);
        assert_eq!(next_read_capacity(4, 10), 4);
        assert_eq!(next_read_capacity(usize::MAX, usize::MAX), u32::MAX);
    }

    #[derive(Clone)]
    enum QueryBehavior {
        Available(u32),
        SyncError,
        CallbackError,
        Malformed,
        Pending,
    }

    #[derive(Clone)]
    enum ReadBehavior {
        Data(Vec<u8>),
        MismatchedAddress,
        SyncError,
        CallbackError,
        Malformed,
        Pending,
    }

    #[derive(Clone)]
    struct ReadStep {
        query: QueryBehavior,
        read: ReadBehavior,
    }

    impl ReadStep {
        fn data(available: u32, data: impl Into<Vec<u8>>) -> Self {
            Self {
                query: QueryBehavior::Available(available),
                read: ReadBehavior::Data(data.into()),
            }
        }
    }

    #[derive(Clone)]
    enum TrailerBehavior {
        None,
        Raw(Vec<u8>),
        Error,
    }

    #[derive(Default)]
    struct Record {
        context: AtomicUsize,
        query_calls: AtomicUsize,
        read_calls: AtomicUsize,
        trailer_queries: AtomicUsize,
        requested: Mutex<Vec<u32>>,
        lent_addresses: Mutex<Vec<usize>>,
        session_closes: AtomicUsize,
        connect_closes: AtomicUsize,
        request_closes: AtomicUsize,
    }

    struct ReaderHarness {
        reader: WinHttpBodyReader,
        memory: GlobalPool,
        context: *mut RequestContext,
        record: Arc<Record>,
    }

    impl ReaderHarness {
        fn into_body(self) -> BodyHarness {
            BodyHarness {
                body: Box::pin(WinHttpResponseBody::new(self.reader)),
                context: self.context,
                record: self.record,
            }
        }

        fn finish(self) {
            drop(self.reader);
            finish_context(self.context, &self.record);
        }
    }

    struct BodyHarness {
        body: Pin<Box<WinHttpResponseBody>>,
        context: *mut RequestContext,
        record: Arc<Record>,
    }

    #[test]
    fn first_poll_is_lazy_and_reads_data_before_authoritative_eof() {
        let harness = reader(
            [ReadStep::data(3, b"abc".to_vec()), ReadStep::data(0, Vec::new())],
            TrailerBehavior::None,
        )
        .into_body();
        let BodyHarness { mut body, context, record } = harness;

        assert_eq!(record.query_calls.load(Ordering::SeqCst), 0);
        assert_eq!(record.read_calls.load(Ordering::SeqCst), 0);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 0);

        let frame = drive(next_frame(&mut body)).unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), b"abc");
        assert_eq!(record.query_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*record.requested.lock().unwrap(), [3]);

        assert!(drive(next_frame(&mut body)).is_none(), "zero READ_COMPLETE establishes EOF");
        assert_eq!(record.query_calls.load(Ordering::SeqCst), 2);
        assert_eq!(record.read_calls.load(Ordering::SeqCst), 2);
        let requested = record.requested.lock().unwrap();
        assert!(requested[1] > 0);
        assert!(requested[1] <= u32::try_from(PREFERRED_READ_SIZE).unwrap());
        drop(requested);
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn a_read_after_end_of_stream_never_reaches_the_transport() {
        let mut harness = reader([ReadStep::data(0, Vec::new())], TrailerBehavior::None);

        let (read, into) = drive(harness.reader.read_into(BytesBuf::new())).unwrap();
        assert_eq!(read, 0, "a zero-length completion is authoritative end-of-stream");
        assert_eq!(harness.record.query_calls.load(Ordering::SeqCst), 1);

        let (read, into) = drive(harness.reader.read_into(into)).unwrap();

        assert_eq!(read, 0);
        assert!(into.peek().is_empty());
        assert_eq!(
            harness.record.query_calls.load(Ordering::SeqCst),
            1,
            "the second read is answered without reaching WinHTTP"
        );
        harness.finish();
    }

    #[test]
    fn a_read_appends_to_the_caller_buffer() {
        let mut harness = reader([ReadStep::data(3, b"abc".to_vec())], TrailerBehavior::None);
        let mut into = harness.memory.reserve(8);
        into.put_slice(*b"prefix");

        let (read, mut into) = drive(harness.reader.read_into(into)).unwrap();

        assert_eq!(read, 3);
        assert_eq!(into.consume_all(), b"prefixabc");
        harness.finish();
    }

    #[test]
    fn a_completion_belonging_to_another_operation_is_rejected() {
        // Each operation slot accepts only its own completion. A completion for
        // a different operation means the callback protocol was violated, which
        // the reader reports instead of interpreting the foreign payload.
        let available = data_available_completion(CompletionResult::SendRequestComplete).unwrap_err();
        assert_eq!(available.label(), "request_winhttp");
        assert!(available.to_string().contains("QueryDataAvailable"), "{available}");

        let read = read_completion(CompletionResult::SendRequestComplete).unwrap_err();
        assert_eq!(read.label(), "request_winhttp");
        assert!(read.to_string().contains("ReadData"), "{read}");
    }

    #[test]
    fn reads_preserve_existing_bytes_across_partial_reads() {
        let mut harness = reader(
            [ReadStep::data(2, b"xy".to_vec()), ReadStep::data(5, b"12345".to_vec())],
            TrailerBehavior::None,
        );
        let mut into = harness.memory.reserve(16);
        into.put_slice(*b"prefix");
        let prefix_address = into.peek().first_slice().as_ptr().addr();

        let (first_read, into) = drive(harness.reader.read_into(into)).unwrap();
        assert_eq!(first_read, 2);
        assert_eq!(into.peek(), b"prefixxy");
        assert_eq!(into.peek().first_slice().as_ptr().addr(), prefix_address);

        let (second_read, mut into) = drive(harness.reader.read_into(into)).unwrap();
        assert_eq!(second_read, 5);
        assert_eq!(into.consume_all(), b"prefixxy12345");
        // Each read submits the span the availability query reported, so the
        // buffer grows by exactly what was readable at that moment.
        assert_eq!(*harness.record.requested.lock().unwrap(), [2, 5]);

        harness.finish();
    }

    #[test]
    fn zero_availability_uses_a_positive_bounded_probe() {
        let mut harness = reader([ReadStep::data(0, b"z".to_vec())], TrailerBehavior::None);

        let (read, data) = drive(harness.reader.read_into(BytesBuf::new())).unwrap();

        assert_eq!(read, 1);
        assert_eq!(data.peek(), b"z");
        // A zero availability result carries no size information, so the read
        // is speculative: it reserves the full upper bound and submits as much
        // of that reserve as the first contiguous region exposes.
        let mut probe = BytesBuf::new();
        probe.reserve(PREFERRED_READ_SIZE, &harness.memory);
        let contiguous = probe.first_unfilled_slice().len().min(PREFERRED_READ_SIZE);
        assert_eq!(harness.record.requested.lock().unwrap()[0], u32::try_from(contiguous).unwrap());
        assert!(data.capacity() >= PREFERRED_READ_SIZE);
        harness.finish();
    }

    #[test]
    fn spare_capacity_serves_later_reads_without_renting_more_memory() {
        let mut harness = reader(
            [ReadStep::data(1, b"a".to_vec()), ReadStep::data(1, b"b".to_vec())],
            TrailerBehavior::None,
        );

        let (_, into) = drive(harness.reader.read_into(BytesBuf::new())).unwrap();
        let spare_after_first = into.remaining_capacity();

        let (read, into) = drive(harness.reader.read_into(into)).unwrap();

        assert_eq!(read, 1);
        // The reserve left by the first read serves the second one, so a peer
        // trickling one byte at a time cannot make the transport rent a pool
        // block per byte.
        assert_eq!(into.remaining_capacity(), spare_after_first - 1);
        harness.finish();
    }

    #[test]
    fn a_small_availability_rents_a_proportionally_small_block() {
        let mut harness = reader([ReadStep::data(3, b"abc".to_vec())], TrailerBehavior::None);

        let (read, data) = drive(harness.reader.read_into(BytesBuf::new())).unwrap();

        assert_eq!(read, 3);
        assert_eq!(data.peek(), b"abc");
        assert_eq!(harness.record.requested.lock().unwrap()[0], 3);
        // GlobalPool picks its block size class from the requested size, so a
        // three-byte availability must not rent a block sized for the
        // speculative upper bound that stays rented for as long as the consumer
        // holds the frame.
        assert!(
            data.capacity() < PREFERRED_READ_SIZE,
            "the reservation must follow the queried availability, not the speculative upper bound"
        );
        harness.finish();
    }

    #[test]
    fn reads_only_the_first_contiguous_region_and_bounds_it_by_u32() {
        let mut harness = reader([ReadStep::data(u32::MAX, b"x".to_vec())], TrailerBehavior::None);
        let mut into = harness.memory.reserve(8);
        let first_region = into.first_unfilled_slice().len();
        assert!(first_region < PREFERRED_READ_SIZE, "the test requires a small first allocation");
        into.reserve(PREFERRED_READ_SIZE, &harness.memory);
        assert!(into.remaining_capacity() >= PREFERRED_READ_SIZE);

        let (read, mut into) = drive(harness.reader.read_into(into)).unwrap();

        assert_eq!(read, 1);
        assert_eq!(into.consume_all(), b"x");
        assert_eq!(harness.record.requested.lock().unwrap()[0], u32::try_from(first_region).unwrap());
        harness.finish();
    }

    #[test]
    fn trailers_preserve_duplicates_and_opaque_values_and_are_emitted_once() {
        let raw_trailers = [
            b"x-trailer: first\r\n".as_slice(),
            b"x-trailer: ".as_slice(),
            &[0x80, 0xff],
            b"\r\n\r\n",
        ]
        .concat();
        let harness = reader(
            [ReadStep::data(4, b"data".to_vec()), ReadStep::data(0, Vec::new())],
            TrailerBehavior::Raw(raw_trailers),
        )
        .into_body();
        let BodyHarness { mut body, context, record } = harness;

        let data = drive(next_frame(&mut body)).unwrap().unwrap().into_data().unwrap();
        assert_eq!(data, b"data");

        let trailers = drive(next_frame(&mut body)).unwrap().unwrap().into_trailers().unwrap();
        assert_eq!(
            trailers.get_all("x-trailer").iter().map(HeaderValue::as_bytes).collect::<Vec<_>>(),
            [b"first".as_slice(), &[0x80, 0xff]]
        );
        assert!(drive(next_frame(&mut body)).is_none());
        assert!(drive(next_frame(&mut body)).is_none());
        assert_eq!(record.trailer_queries.load(Ordering::SeqCst), 2);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn missing_trailers_produce_no_frame() {
        let harness = reader([ReadStep::data(0, Vec::new())], TrailerBehavior::None).into_body();
        let BodyHarness { mut body, context, record } = harness;

        assert!(drive(next_frame(&mut body)).is_none());
        assert_eq!(record.trailer_queries.load(Ordering::SeqCst), 1);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn empty_trailer_block_emits_one_empty_trailer_frame() {
        let harness = reader([ReadStep::data(0, Vec::new())], TrailerBehavior::Raw(b"\r\n".to_vec())).into_body();
        let BodyHarness { mut body, context, record } = harness;

        assert!(!body.is_end_stream());
        let trailers = drive(next_frame(&mut body)).unwrap().unwrap().into_trailers().unwrap();
        assert!(trailers.is_empty());
        assert!(body.is_end_stream());
        assert!(drive(next_frame(&mut body)).is_none());
        assert_eq!(record.trailer_queries.load(Ordering::SeqCst), 2);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn malformed_or_failed_trailer_queries_fail_and_close_the_request() {
        for trailers in [TrailerBehavior::Raw(b"malformed\r\n\r\n".to_vec()), TrailerBehavior::Error] {
            let harness = reader([ReadStep::data(0, Vec::new())], trailers).into_body();
            let BodyHarness { mut body, context, record } = harness;

            let error = drive(next_frame(&mut body)).unwrap().unwrap_err();
            assert_eq!(error.label(), "request_winhttp");
            assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);

            drop(body);
            finish_context(context, &record);
        }
    }

    #[test]
    fn synchronous_callback_and_malformed_read_failures_close_the_request() {
        let cases = [
            ReadStep {
                query: QueryBehavior::SyncError,
                read: ReadBehavior::Pending,
            },
            ReadStep {
                query: QueryBehavior::CallbackError,
                read: ReadBehavior::Pending,
            },
            ReadStep {
                query: QueryBehavior::Malformed,
                read: ReadBehavior::Pending,
            },
            ReadStep {
                query: QueryBehavior::Available(1),
                read: ReadBehavior::SyncError,
            },
            ReadStep {
                query: QueryBehavior::Available(1),
                read: ReadBehavior::CallbackError,
            },
            ReadStep {
                query: QueryBehavior::Available(1),
                read: ReadBehavior::Malformed,
            },
        ];

        for step in cases {
            let mut harness = reader([step], TrailerBehavior::None);
            let error = drive(harness.reader.read_into(BytesBuf::new())).unwrap_err();
            assert_eq!(error.label(), "request_winhttp");
            assert_eq!(harness.record.request_closes.load(Ordering::SeqCst), 1);
            harness.finish();
        }
    }

    #[test]
    fn a_read_completion_reporting_a_foreign_buffer_address_is_rejected() {
        let mut harness = reader(
            [ReadStep {
                query: QueryBehavior::Available(1),
                read: ReadBehavior::MismatchedAddress,
            }],
            TrailerBehavior::None,
        );

        let error = drive(harness.reader.read_into(BytesBuf::new())).unwrap_err();

        assert_eq!(error.label(), "request_winhttp");
        assert!(error.to_string().contains("invalid status information"), "{error}");
        assert_eq!(harness.record.request_closes.load(Ordering::SeqCst), 1);
        harness.finish();
    }

    #[test]
    fn a_mid_stream_error_terminates_the_body() {
        let harness = reader(
            [
                ReadStep::data(1, b"a".to_vec()),
                ReadStep {
                    query: QueryBehavior::Available(1),
                    read: ReadBehavior::CallbackError,
                },
            ],
            TrailerBehavior::None,
        )
        .into_body();
        let BodyHarness { mut body, context, record } = harness;

        let data = drive(next_frame(&mut body)).unwrap().unwrap().into_data().unwrap();
        assert_eq!(data, b"a");
        drive(next_frame(&mut body)).unwrap().unwrap_err();
        assert!(drive(next_frame(&mut body)).is_none());
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn dropping_before_poll_or_during_query_or_read_defers_parent_reclamation() {
        for step in [
            None,
            Some(ReadStep {
                query: QueryBehavior::Pending,
                read: ReadBehavior::Pending,
            }),
            Some(ReadStep {
                query: QueryBehavior::Available(1),
                read: ReadBehavior::Pending,
            }),
        ] {
            let should_poll = step.is_some();
            let harness = reader(step, TrailerBehavior::None).into_body();
            let BodyHarness { mut body, context, record } = harness;

            if should_poll {
                let mut cx = Context::from_waker(Waker::noop());
                assert!(body.as_mut().poll_frame(&mut cx).is_pending());
            }

            drop(body);
            assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
            assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
            assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);

            finish_context(context, &record);
        }
    }

    #[test]
    fn a_deferred_read_resumes_and_delivers_its_frame_on_a_later_poll() {
        // WinHTTP normally answers on a callback thread after the poll that
        // submitted the operation has already returned Pending, so the frame is
        // assembled on a later poll rather than inline with the submission.
        let harness = reader(
            [
                ReadStep {
                    query: QueryBehavior::Pending,
                    read: ReadBehavior::Data(b"abc".to_vec()),
                },
                ReadStep::data(0, Vec::new()),
            ],
            TrailerBehavior::None,
        )
        .into_body();
        let BodyHarness { mut body, context, record } = harness;

        let mut cx = Context::from_waker(Waker::noop());
        assert!(body.as_mut().poll_frame(&mut cx).is_pending());
        assert!(!body.is_end_stream());
        assert_eq!(record.read_calls.load(Ordering::SeqCst), 0);

        let mut available: u32 = 3;
        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload readable and unmodified for the call, no overlapping
        // notification, no outstanding exclusive borrow, and no use of the
        // context after the reclaiming notification. The mock script (`reader`)
        // installed this context and nothing has reclaimed it; the deferred
        // availability query left no notification in flight, and the body is
        // parked between polls so it holds no borrow; and the payload is the
        // initialized local `available`, which outlives the call and nothing
        // else can reach.
        unsafe {
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                (&raw mut available).cast(),
                status_info_len::<u32>(),
            );
        }

        let frame = drive(next_frame(&mut body)).unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), b"abc");
        assert_eq!(record.read_calls.load(Ordering::SeqCst), 1);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn consecutive_frames_refill_the_retained_block_instead_of_renting_another() {
        // A peer that trickles data is the case that makes buffer retention
        // matter: each frame is far smaller than the block that backs it.
        let harness = reader(
            [
                ReadStep::data(3, b"abc".to_vec()),
                ReadStep::data(2, b"de".to_vec()),
                ReadStep::data(0, Vec::new()),
            ],
            TrailerBehavior::None,
        )
        .into_body();
        let BodyHarness { mut body, context, record } = harness;

        let first = drive(next_frame(&mut body)).unwrap().unwrap();
        assert_eq!(first.into_data().unwrap(), b"abc");
        let second = drive(next_frame(&mut body)).unwrap().unwrap();
        assert_eq!(second.into_data().unwrap(), b"de");
        assert!(drive(next_frame(&mut body)).is_none());

        // The second read must be lent the region immediately following the
        // bytes the first frame took, which only holds while the adapter keeps
        // the buffer. Renting a fresh block per frame would place it elsewhere.
        let addresses = record.lent_addresses.lock().unwrap();
        assert_eq!(addresses.len(), 3);
        assert_eq!(addresses[1], addresses[0] + 3);
        assert_eq!(addresses[2], addresses[1] + 2);
        drop(addresses);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn a_failed_read_ends_the_stream_without_carrying_capacity_forward() {
        let harness = reader(
            [
                ReadStep::data(3, b"abc".to_vec()),
                ReadStep {
                    query: QueryBehavior::Available(3),
                    read: ReadBehavior::SyncError,
                },
            ],
            TrailerBehavior::None,
        )
        .into_body();
        let BodyHarness { mut body, context, record } = harness;

        let first = drive(next_frame(&mut body)).unwrap().unwrap();
        assert_eq!(first.into_data().unwrap(), b"abc");
        let error = drive(next_frame(&mut body)).unwrap().unwrap_err();
        assert_eq!(error.label(), "request_winhttp");
        assert!(body.is_end_stream());
        assert!(drive(next_frame(&mut body)).is_none());

        drop(body);
        finish_context(context, &record);
    }

    async fn next_frame(
        body: &mut Pin<Box<WinHttpResponseBody>>,
    ) -> Option<Result<http_body::Frame<bytesbuf::BytesView>, fetch::HttpError>> {
        poll_fn(|cx| body.as_mut().poll_frame(cx)).await
    }

    /// Builds a body reader over a mock `WinHTTP` that follows `steps`.
    ///
    /// The script this installs is what discharges the obligations of
    /// [`complete`] for every notification it delivers. Each notification is
    /// raised from inside a binding the reader itself called, so it runs on the
    /// submitting thread and can overlap no other notification for the context;
    /// the context is the one recorded here at installation, and only
    /// [`finish_context`] reclaims it, after the harness and the guard it owns
    /// are dropped; and the harness reaches the context solely through the
    /// reader, which holds only a shared borrow of it.
    #[expect(
        clippy::too_many_lines,
        reason = "the body-reader harness keeps one complete WinHTTP script visible in one place"
    )]
    fn reader(steps: impl IntoIterator<Item = ReadStep>, trailers: TrailerBehavior) -> ReaderHarness {
        let steps = Arc::new(Mutex::new(steps.into_iter().collect::<VecDeque<_>>()));
        let record = Arc::new(Record::default());
        let mut bindings = MockBindings::new();

        let option_record = Arc::clone(&record);
        bindings.expect_set_option().returning(move |handle, option, value| {
            assert_eq!(handle.as_ptr().addr(), REQUEST);
            assert_eq!(option, WINHTTP_OPTION_CONTEXT_VALUE);
            let context = usize::from_ne_bytes(value.try_into().unwrap());
            option_record.context.store(context, Ordering::SeqCst);
            Ok(())
        });

        let query_steps = Arc::clone(&steps);
        let query_record = Arc::clone(&record);
        bindings.expect_query_data_available().returning(move |_| {
            query_record.query_calls.fetch_add(1, Ordering::SeqCst);
            let query = query_steps.lock().unwrap().front().unwrap().query.clone();
            let context = recorded_context(&query_record);

            match query {
                QueryBehavior::Available(mut available) => {
                    // SAFETY: complete requires an installed, not-yet-reclaimed
                    // context, a payload readable and unmodified for the call,
                    // no overlapping notification, no outstanding exclusive
                    // borrow, and no use of the context after the reclaiming
                    // notification. The mock script (`reader`) establishes all
                    // of them; the payload here is the initialized local
                    // `available`, which outlives the call and nothing else can
                    // reach.
                    unsafe {
                        complete(
                            context,
                            WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                            (&raw mut available).cast(),
                            status_info_len::<u32>(),
                        );
                    }
                    Ok(())
                }
                QueryBehavior::SyncError => Err(WinHttpError::new(12030, WinHttpOperation::QueryDataAvailable)),
                QueryBehavior::CallbackError => {
                    // SAFETY: complete_request_error requires an installed,
                    // not-yet-reclaimed context, no overlapping notification,
                    // no outstanding exclusive borrow, and no use of the
                    // context after the reclaiming notification, and it
                    // supplies the payload itself. The mock script (`reader`)
                    // establishes all of them.
                    unsafe {
                        complete_request_error(context, 12030);
                    }
                    Ok(())
                }
                QueryBehavior::Malformed => {
                    // SAFETY: complete requires an installed, not-yet-reclaimed
                    // context, a payload matching the notification, no
                    // overlapping notification, no outstanding exclusive
                    // borrow, and no use of the context after the reclaiming
                    // notification. The mock script (`reader`) establishes all
                    // of them; a null pointer of zero length is admitted for
                    // any status, which is what makes this completion malformed
                    // for one that must carry a `DWORD`.
                    unsafe {
                        complete(context, WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, std::ptr::null_mut(), 0);
                    }
                    Ok(())
                }
                QueryBehavior::Pending => Ok(()),
            }
        });

        let read_steps = Arc::clone(&steps);
        let read_record = Arc::clone(&record);
        bindings.expect_read_data().returning(move |_, buffer, len| {
            read_record.read_calls.fetch_add(1, Ordering::SeqCst);
            read_record.requested.lock().unwrap().push(len);
            read_record.lent_addresses.lock().unwrap().push(buffer.as_ptr().addr());
            let step = read_steps.lock().unwrap().pop_front().unwrap();
            let context = recorded_context(&read_record);

            match step.read {
                ReadBehavior::Data(data) => {
                    assert!(data.len() <= len as usize);
                    // SAFETY: the reader lent this pointer as the start of a
                    // writable region of `len` bytes that the active operation
                    // retains, so the destination is valid for the bytes copied
                    // here and no other reference to it exists while this mock
                    // stands in for WinHTTP. The source is a separate allocation,
                    // so the regions cannot overlap, and `u8` imposes no
                    // alignment requirement. The region is uninitialized memory,
                    // which is why it is filled through the raw pointer instead
                    // of through a slice reference.
                    unsafe {
                        buffer.as_ptr().copy_from_nonoverlapping(data.as_ptr(), data.len());
                    }
                    if data.is_empty() {
                        // SAFETY: complete requires an installed,
                        // not-yet-reclaimed context, a payload matching the
                        // notification, no overlapping notification, no
                        // outstanding exclusive borrow, and no use of the
                        // context after the reclaiming notification. The mock
                        // script (`reader`) establishes all of them; a read
                        // that returned nothing reports a null address and a
                        // zero count, which `WinHTTP` uses to signal the end of
                        // the response body.
                        unsafe {
                            complete(context, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, std::ptr::null_mut(), 0);
                        }
                    } else {
                        // SAFETY: complete requires an installed,
                        // not-yet-reclaimed context, a payload matching the
                        // notification, no overlapping notification, no
                        // outstanding exclusive borrow, and no use of the
                        // context after the reclaiming notification. The mock
                        // script (`reader`) establishes all of them. This
                        // notification reports the lent buffer and the count of
                        // bytes written into it, and the copy above initialized
                        // exactly that many bytes there.
                        unsafe {
                            complete(
                                context,
                                WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
                                buffer.as_ptr().cast(),
                                u32::try_from(data.len()).unwrap(),
                            );
                        }
                    }
                    Ok(())
                }
                ReadBehavior::MismatchedAddress => {
                    // A completion reporting an address other than the submitted
                    // buffer cannot describe bytes written into that buffer. The
                    // reported length stays within the submitted capacity so the
                    // address is the only reason to reject this completion.
                    //
                    // SAFETY: complete requires an installed, not-yet-reclaimed
                    // context, a payload matching the notification, no
                    // overlapping notification, no outstanding exclusive
                    // borrow, and no use of the context after the reclaiming
                    // notification. The mock script (`reader`) establishes all
                    // of them, and the payload obligation exempts
                    // `READ_COMPLETE`, whose status info states an address and
                    // byte count that are compared against the submitted buffer
                    // rather than dereferenced, which is what lets this one
                    // describe bytes no read wrote.
                    unsafe {
                        complete(
                            context,
                            WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
                            buffer.as_ptr().wrapping_add(1).cast(),
                            1,
                        );
                    }
                    Ok(())
                }
                ReadBehavior::SyncError => Err(WinHttpError::new(12030, WinHttpOperation::ReadData)),
                ReadBehavior::CallbackError => {
                    // SAFETY: complete_request_error requires an installed,
                    // not-yet-reclaimed context, no overlapping notification,
                    // no outstanding exclusive borrow, and no use of the
                    // context after the reclaiming notification, and it
                    // supplies the payload itself. The mock script (`reader`)
                    // establishes all of them.
                    unsafe {
                        complete_request_error(context, 12030);
                    }
                    Ok(())
                }
                ReadBehavior::Malformed => {
                    // SAFETY: complete requires an installed, not-yet-reclaimed
                    // context, a payload matching the notification, no
                    // overlapping notification, no outstanding exclusive
                    // borrow, and no use of the context after the reclaiming
                    // notification. The mock script (`reader`) establishes all
                    // of them, and the payload obligation exempts
                    // `READ_COMPLETE`, whose status info states an address and
                    // byte count that are compared against the submitted buffer
                    // rather than dereferenced, which is what lets this one
                    // claim more bytes than the lent buffer holds.
                    unsafe {
                        complete(
                            context,
                            WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
                            buffer.as_ptr().cast(),
                            len.saturating_add(1),
                        );
                    }
                    Ok(())
                }
                ReadBehavior::Pending => Ok(()),
            }
        });

        let trailer_record = Arc::clone(&record);
        bindings.expect_query_headers().returning(move |_, _, buffer, byte_len| {
            trailer_record.trailer_queries.fetch_add(1, Ordering::SeqCst);
            match &trailers {
                TrailerBehavior::None => Err(WinHttpError::new(ERROR_WINHTTP_HEADER_NOT_FOUND, WinHttpOperation::QueryHeaders)),
                TrailerBehavior::Raw(raw) => write_byte_query(raw, buffer, byte_len),
                TrailerBehavior::Error => Err(WinHttpError::new(12152, WinHttpOperation::QueryHeaders)),
            }
        });

        let close_record = Arc::clone(&record);
        bindings.expect_close_handle().returning(move |handle| {
            match handle.as_ptr().addr() {
                SESSION => &close_record.session_closes,
                CONNECT => &close_record.connect_closes,
                REQUEST => &close_record.request_closes,
                _ => panic!("unexpected handle"),
            }
            .fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let facade = BindingsFacade::mock(Arc::new(bindings));
        let session = Arc::new(WinHttpSession::from_handle(SessionHandle::new(raw_handle(SESSION), facade.clone())));
        let contexts = ContextPool::new(Pool::new());
        let guard = ContextInstallation::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade.clone()),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .unwrap();
        let context = installed_context(&guard);
        drop((session, contexts));
        let memory = GlobalPool::new();

        ReaderHarness {
            reader: WinHttpBodyReader::new(guard, facade, memory.clone()),
            memory,
            context,
            record,
        }
    }

    fn finish_context(context: *mut RequestContext, record: &Record) {
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);

        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no use
        // of the pointer or of a guard holding it afterwards. `reader` recorded
        // this pointer at installation and nothing has reclaimed it; every
        // caller drops the harness, and with it the guard the reader owns,
        // before calling here, so no notification is in flight and none can
        // follow; and the harness borrows the context only sharedly.
        unsafe {
            closing(context);
        }

        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    /// Reads the context pointer the mock bindings recorded at installation.
    fn recorded_context(record: &Record) -> *mut RequestContext {
        context_pointer(record.context.load(Ordering::SeqCst))
    }

    fn write_byte_query(bytes: &[u8], buffer: Option<NonNull<u8>>, byte_len: &mut u32) -> crate::error::Result<()> {
        let required = u32::try_from(bytes.len().checked_add(1).unwrap()).unwrap();
        let Some(output) = buffer else {
            *byte_len = required;
            return Err(WinHttpError::new(ERROR_INSUFFICIENT_BUFFER.0, WinHttpOperation::QueryHeaders));
        };

        assert!(*byte_len >= required);
        // SAFETY: the sizing query reported `required` and the caller asserted
        // at least that much writable space, so the destination holds the copied
        // content. It is a separate allocation from `bytes`, so the regions
        // cannot overlap, and `u8` imposes no alignment requirement.
        unsafe { output.as_ptr().copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        // SAFETY: `required` counts one writable byte beyond the copied content,
        // so this offset stays inside the same allocation.
        let terminator = unsafe { output.as_ptr().add(bytes.len()) };
        // SAFETY: `terminator` addresses that final writable byte.
        unsafe { terminator.write(0) };
        *byte_len = u32::try_from(bytes.len()).unwrap();
        Ok(())
    }
}
