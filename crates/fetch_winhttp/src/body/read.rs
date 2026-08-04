// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;
use std::task::{Context, Poll};

use bytesbuf::BytesBuf;
use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared};
use bytesbuf_io::{Read, ReadAsFuturesStream, ReadExt as _};
use fetch::HttpError;
use futures_core::Stream as _;
use http::HeaderMap;
use http_body::{Body, Frame, SizeHint};

use crate::bindings::{Bindings as _, BindingsFacade};
use crate::context::{CompletionResult, OperationBuffer, OperationKind};
use crate::error::{callback_protocol_error, invalid_response, query_error};
use crate::operation::RequestGuard;
use crate::query::query_raw_trailers;
use crate::response_headers::parse_response_trailers;

// When WinHTTP reports zero available bytes, it can still complete a useful
// read. GlobalPool's largest pooled block is 64 KiB, so use that pool-aligned
// upper bound for this speculative read. The caller's limit and any existing
// writable tail can reduce the span actually submitted. Revisit this value if
// the pool's size classes or measured throughput-to-memory trade-off changes.
const PREFERRED_READ_SIZE: usize = 64 * 1024;

#[derive(Debug)]
/// Pulls one response body lazily from an ongoing WinHTTP request.
///
/// Each caller-driven read first queries available data and then lends one
/// contiguous writable span from a pooled `BytesBuf` to WinHTTP. Capacity is
/// reserved only after that query, so the rented pool block matches the bytes
/// actually readable instead of a fixed maximum; a trickling peer therefore
/// cannot amplify a small response into many large rented blocks. The retained
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

    async fn read_into(&mut self, limit: usize, into: BytesBuf) -> Result<(usize, BytesBuf), HttpError> {
        if limit == 0 || self.eof {
            return Ok((0, into));
        }

        let result = self.read_once(limit, into).await;
        if result.is_err() {
            drop(self.guard.take());
        }
        result
    }

    async fn read_once(&mut self, limit: usize, mut into: BytesBuf) -> Result<(usize, BytesBuf), HttpError> {
        let available = {
            let bindings = self.bindings.clone();
            let query = self
                .guard_mut()?
                .submit(OperationKind::DataAvailable, OperationBuffer::none(), move |request, _context| {
                    // SAFETY: submit() armed the data-available operation and
                    // transferred the live request handle into its future. The
                    // installed callback context remains valid and no operation
                    // overlaps this query.
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
        .min(limit);
        into.reserve(desired, &self.memory);

        let (buffer, address, capacity) = {
            let tail = into.first_unfilled_slice();
            let capacity = next_read_capacity(tail.len(), desired);
            debug_assert!(capacity != 0, "a positive reservation must expose a writable tail");
            let buffer = NonNull::new(tail.as_mut_ptr().cast::<u8>()).expect("a positive writable tail has a non-null pointer");
            (buffer, buffer.as_ptr().addr(), capacity)
        };

        let bindings = self.bindings.clone();
        let read = self.guard_mut()?.submit(
            OperationKind::Read,
            OperationBuffer::read(into, address, capacity),
            move |request, _context| {
                // SAFETY: submit() armed the read operation and transferred the
                // live request handle into its future. The active operation
                // owns the same BytesBuf and contiguous writable tail until
                // completion, and no operation overlaps this read.
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
            // SAFETY: callback validation established that exactly `read`
            // bytes at the start of the exposed writable tail were initialized.
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

impl Memory for WinHttpBodyReader {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

impl HasMemory for WinHttpBodyReader {
    fn memory(&self) -> impl MemoryShared {
        self.memory.clone()
    }
}

impl Read for WinHttpBodyReader {
    type Error = HttpError;

    async fn read_at_most_into(&mut self, len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        self.read_into(len, into).await
    }

    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        self.read_into(PREFERRED_READ_SIZE, into).await
    }

    async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
        // No capacity is reserved here on purpose. `GlobalPool` picks its block
        // size class from the requested size, so reserving `PREFERRED_READ_SIZE`
        // before `WinHttpQueryDataAvailable` reports availability would rent a
        // 64 KiB block for every frame, however small. Each returned frame keeps
        // its block rented for as long as the consumer holds the view, so a peer
        // that trickles data would amplify a small response into many full
        // blocks - the hazard documented under `Read::read_any`'s "Security"
        // heading and contrary to implementation.md section 6.2. `read_once`
        // reserves after the availability query instead, so the block size
        // class matches the bytes that are actually readable.
        self.read_into(PREFERRED_READ_SIZE, BytesBuf::new()).await.map(|(_read, into)| into)
    }
}

/// Preserves WinHTTP data and trailers as `http_body` frames.
///
/// A dedicated adapter is required because the generic byte-stream bridge
/// cannot emit response trailers. It yields data lazily, emits at most one
/// final trailer frame, and disposes of the reader after completion or error.
/// The surrounding `HttpBodyBuilder` remains responsible for applying the
/// configured body idle timeout.
pub(crate) struct WinHttpResponseBody {
    stream: Option<Pin<Box<ReadAsFuturesStream<WinHttpBodyReader>>>>,
    done: bool,
}

impl WinHttpResponseBody {
    pub(crate) fn new(reader: WinHttpBodyReader) -> Self {
        Self {
            stream: Some(reader.into_futures_stream()),
            done: false,
        }
    }
}

impl fmt::Debug for WinHttpResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WinHttpResponseBody")
            .field("stream", &self.stream)
            .field("done", &self.done)
            .finish()
    }
}

impl Body for WinHttpResponseBody {
    type Data = bytesbuf::BytesView;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.done {
            return Poll::Ready(None);
        }

        let stream = self
            .stream
            .as_mut()
            .expect("a response body that is not done retains its reader stream");
        match stream.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(data))) => Poll::Ready(Some(Ok(Frame::data(data)))),
            Poll::Ready(Some(Err(error))) => {
                self.stream = None;
                self.done = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                let stream = self.stream.take().expect("the completed response body retains its reader stream");
                let mut reader = stream.into_inner();
                let trailers = reader.take_trailers();
                self.done = true;

                Poll::Ready(trailers.map(|trailers| Ok(Frame::trailers(trailers))))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
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

fn next_read_capacity(tail_len: usize, desired: usize) -> u32 {
    u32::try_from(tail_len.min(desired).min(u32::MAX as usize)).expect("the read capacity is bounded by u32::MAX")
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::future::poll_fn;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::pin::Pin;
    use std::ptr::NonNull;
    use std::slice;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Waker};

    use bytesbuf::mem::{HasMemory as _, Memory as _};
    use bytesbuf_io::Read as _;
    use http::HeaderValue;
    use http_body::Body as _;
    use ohno::Labeled as _;
    use plurality::Pool;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::Networking::WinHttp::{
        ERROR_WINHTTP_HEADER_NOT_FOUND, WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
    };

    use super::{PREFERRED_READ_SIZE, WinHttpBodyReader, WinHttpResponseBody, next_read_capacity};
    use crate::bindings::{BindingsFacade, MockBindings, WINHTTP_OPTION_CONTEXT_VALUE};
    use crate::callback::dispatch_completion;
    use crate::context::RequestContext;
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RawHandle, RequestHandle, SessionHandle};
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
    fn read_capacity_obeys_empty_caller_tail_and_dword_bounds() {
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
        session_closes: AtomicUsize,
        connect_closes: AtomicUsize,
        request_closes: AtomicUsize,
    }

    struct ReaderHarness {
        reader: WinHttpBodyReader,
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

        let frame = futures::executor::block_on(next_frame(&mut body)).unwrap().unwrap();
        assert_eq!(frame.into_data().unwrap(), b"abc");
        assert_eq!(record.query_calls.load(Ordering::SeqCst), 1);
        assert_eq!(record.read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*record.requested.lock().unwrap(), [3]);

        assert!(
            futures::executor::block_on(next_frame(&mut body)).is_none(),
            "zero READ_COMPLETE establishes EOF"
        );
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
    fn reads_preserve_existing_bytes_and_obey_caller_limits_across_partial_reads() {
        let mut harness = reader(
            [ReadStep::data(20, b"xy".to_vec()), ReadStep::data(20, b"12345".to_vec())],
            TrailerBehavior::None,
        );
        let memory = harness.reader.memory();
        let mut into = memory.reserve(16);
        into.put_slice(*b"prefix");
        let prefix_address = into.peek().first_slice().as_ptr().addr();

        let (first_read, into) = futures::executor::block_on(harness.reader.read_at_most_into(3, into)).unwrap();
        assert_eq!(first_read, 2);
        assert_eq!(into.peek(), b"prefixxy");
        assert_eq!(into.peek().first_slice().as_ptr().addr(), prefix_address);

        let (second_read, mut into) = futures::executor::block_on(harness.reader.read_at_most_into(5, into)).unwrap();
        assert_eq!(second_read, 5);
        assert_eq!(into.consume_all(), b"prefixxy12345");
        assert_eq!(*harness.record.requested.lock().unwrap(), [3, 5]);

        harness.finish();
    }

    #[test]
    fn zero_availability_uses_a_positive_bounded_probe() {
        let mut harness = reader([ReadStep::data(0, b"z".to_vec())], TrailerBehavior::None);

        let data = futures::executor::block_on(harness.reader.read_any()).unwrap();

        assert_eq!(data.peek(), b"z");
        // A zero availability result carries no size information, so the
        // speculative read still reserves the full pool-aligned upper bound.
        assert_eq!(
            harness.record.requested.lock().unwrap()[0],
            u32::try_from(PREFERRED_READ_SIZE).unwrap()
        );
        assert!(data.capacity() >= PREFERRED_READ_SIZE);
        harness.finish();
    }

    #[test]
    fn a_small_availability_rents_a_proportionally_small_block() {
        let mut harness = reader([ReadStep::data(3, b"abc".to_vec())], TrailerBehavior::None);

        let data = futures::executor::block_on(harness.reader.read_any()).unwrap();

        assert_eq!(data.peek(), b"abc");
        assert_eq!(harness.record.requested.lock().unwrap()[0], 3);
        // GlobalPool picks its block size class from the requested size, so a
        // three-byte availability must not rent a 64 KiB block that stays
        // rented for as long as the consumer holds the frame.
        assert!(
            data.capacity() < PREFERRED_READ_SIZE,
            "the reservation must follow the queried availability, not the speculative upper bound"
        );
        harness.finish();
    }

    #[test]
    fn reads_only_the_first_contiguous_tail_and_bounds_it_by_u32() {
        let mut harness = reader([ReadStep::data(u32::MAX, b"x".to_vec())], TrailerBehavior::None);
        let memory = harness.reader.memory();
        let mut into = memory.reserve(8);
        let first_tail = into.first_unfilled_slice().len();
        assert!(first_tail < PREFERRED_READ_SIZE, "the test requires a small first allocation");
        into.reserve(PREFERRED_READ_SIZE, &memory);
        assert!(into.remaining_capacity() >= PREFERRED_READ_SIZE);

        let (read, mut into) = futures::executor::block_on(harness.reader.read_at_most_into(PREFERRED_READ_SIZE, into)).unwrap();

        assert_eq!(read, 1);
        assert_eq!(into.consume_all(), b"x");
        assert_eq!(harness.record.requested.lock().unwrap()[0], u32::try_from(first_tail).unwrap());
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

        let data = futures::executor::block_on(next_frame(&mut body))
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(data, b"data");

        let trailers = futures::executor::block_on(next_frame(&mut body))
            .unwrap()
            .unwrap()
            .into_trailers()
            .unwrap();
        assert_eq!(
            trailers.get_all("x-trailer").iter().map(HeaderValue::as_bytes).collect::<Vec<_>>(),
            [b"first".as_slice(), &[0x80, 0xff]]
        );
        assert!(futures::executor::block_on(next_frame(&mut body)).is_none());
        assert!(futures::executor::block_on(next_frame(&mut body)).is_none());
        assert_eq!(record.trailer_queries.load(Ordering::SeqCst), 2);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn missing_trailers_produce_no_frame() {
        let harness = reader([ReadStep::data(0, Vec::new())], TrailerBehavior::None).into_body();
        let BodyHarness { mut body, context, record } = harness;

        assert!(futures::executor::block_on(next_frame(&mut body)).is_none());
        assert_eq!(record.trailer_queries.load(Ordering::SeqCst), 1);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn empty_trailer_block_emits_one_empty_trailer_frame() {
        let harness = reader([ReadStep::data(0, Vec::new())], TrailerBehavior::Raw(b"\r\n".to_vec())).into_body();
        let BodyHarness { mut body, context, record } = harness;

        assert!(!body.is_end_stream());
        let trailers = futures::executor::block_on(next_frame(&mut body))
            .unwrap()
            .unwrap()
            .into_trailers()
            .unwrap();
        assert!(trailers.is_empty());
        assert!(body.is_end_stream());
        assert!(futures::executor::block_on(next_frame(&mut body)).is_none());
        assert_eq!(record.trailer_queries.load(Ordering::SeqCst), 2);

        drop(body);
        finish_context(context, &record);
    }

    #[test]
    fn malformed_or_failed_trailer_queries_fail_and_close_the_request() {
        for trailers in [TrailerBehavior::Raw(b"malformed\r\n\r\n".to_vec()), TrailerBehavior::Error] {
            let harness = reader([ReadStep::data(0, Vec::new())], trailers).into_body();
            let BodyHarness { mut body, context, record } = harness;

            let error = futures::executor::block_on(next_frame(&mut body)).unwrap().unwrap_err();
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
            let error = futures::executor::block_on(harness.reader.read_any()).unwrap_err();
            assert_eq!(error.label(), "request_winhttp");
            assert_eq!(harness.record.request_closes.load(Ordering::SeqCst), 1);
            harness.finish();
        }
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

        let data = futures::executor::block_on(next_frame(&mut body))
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(data, b"a");
        futures::executor::block_on(next_frame(&mut body)).unwrap().unwrap_err();
        assert!(futures::executor::block_on(next_frame(&mut body)).is_none());
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

    async fn next_frame(
        body: &mut Pin<Box<WinHttpResponseBody>>,
    ) -> Option<Result<http_body::Frame<bytesbuf::BytesView>, fetch::HttpError>> {
        poll_fn(|cx| body.as_mut().poll_frame(cx)).await
    }

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
            let context = query_record.context.load(Ordering::SeqCst);

            match query {
                QueryBehavior::Available(mut available) => {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                        (&raw mut available).cast(),
                        status_info_len::<u32>(),
                    );
                    Ok(())
                }
                QueryBehavior::SyncError => Err(WinHttpError::new(12030, WinHttpOperation::QueryDataAvailable)),
                QueryBehavior::CallbackError => {
                    complete_request_error(context, 12030);
                    Ok(())
                }
                QueryBehavior::Malformed => {
                    complete(context, WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, std::ptr::null_mut(), 0);
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
            let step = read_steps.lock().unwrap().pop_front().unwrap();
            let context = read_record.context.load(Ordering::SeqCst);

            match step.read {
                ReadBehavior::Data(data) => {
                    assert!(data.len() <= len as usize);
                    // SAFETY: the active operation exposes this exact writable
                    // contiguous tail for `len` bytes.
                    unsafe {
                        slice::from_raw_parts_mut(buffer.as_ptr(), len as usize)[..data.len()].copy_from_slice(&data);
                    }
                    if data.is_empty() {
                        complete(context, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, std::ptr::null_mut(), 0);
                    } else {
                        complete(
                            context,
                            WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
                            buffer.as_ptr().cast(),
                            u32::try_from(data.len()).unwrap(),
                        );
                    }
                    Ok(())
                }
                ReadBehavior::SyncError => Err(WinHttpError::new(12030, WinHttpOperation::ReadData)),
                ReadBehavior::CallbackError => {
                    complete_request_error(context, 12030);
                    Ok(())
                }
                ReadBehavior::Malformed => {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
                        buffer.as_ptr().cast(),
                        len.saturating_add(1),
                    );
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
        let context = guard.context_ptr();
        drop((session, contexts));

        ReaderHarness {
            reader: WinHttpBodyReader::new(guard, facade, bytesbuf::mem::GlobalPool::new()),
            context,
            record,
        }
    }

    fn finish_context(context: *mut RequestContext, record: &Record) {
        assert_eq!(record.request_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 0);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 0);

        // SAFETY: this is the live installed context pointer, and the synthetic
        // HANDLE_CLOSING callback is its final use.
        unsafe {
            dispatch_completion(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
        }

        assert_eq!(record.connect_closes.load(Ordering::SeqCst), 1);
        assert_eq!(record.session_closes.load(Ordering::SeqCst), 1);
    }

    fn complete(context: usize, status: u32, status_info: *mut c_void, status_info_len: u32) {
        // SAFETY: the harness stores the live installed context and follows the
        // callback protocol for every synthetic completion.
        unsafe {
            dispatch_completion(std::ptr::with_exposed_provenance_mut(context), status, status_info, status_info_len);
        }
    }

    fn complete_request_error(context: usize, code: u32) {
        let mut result = WINHTTP_ASYNC_RESULT {
            dwResult: 0,
            dwError: code,
        };
        complete(
            context,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            (&raw mut result).cast(),
            status_info_len::<WINHTTP_ASYNC_RESULT>(),
        );
    }

    fn write_byte_query(bytes: &[u8], buffer: Option<NonNull<u8>>, byte_len: &mut u32) -> crate::error::Result<()> {
        let required = u32::try_from(bytes.len().checked_add(1).unwrap()).unwrap();
        let Some(output) = buffer else {
            *byte_len = required;
            return Err(WinHttpError::new(ERROR_INSUFFICIENT_BUFFER.0, WinHttpOperation::QueryHeaders));
        };

        assert!(*byte_len >= required);
        // SAFETY: the sizing query reserved `required` writable bytes.
        unsafe { output.as_ptr().copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
        // SAFETY: `required` includes one writable byte after the copied data.
        let terminator = unsafe { output.as_ptr().add(bytes.len()) };
        // SAFETY: `terminator` points to the final writable byte.
        unsafe { terminator.write(0) };
        *byte_len = u32::try_from(bytes.len()).unwrap();
        Ok(())
    }

    fn status_info_len<T>() -> u32 {
        u32::try_from(size_of::<T>()).unwrap()
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(value as *mut c_void).unwrap()
    }
}
