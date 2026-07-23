// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Existing contiguous response construction compared with two generated,
// domain-typed prototypes. The Hyper HTTP/1 observation uses an in-memory IO
// sink so body polling, vectored writes, and direct static spans stay visible.

use std::cell::{Cell, RefCell};
use std::convert::Infallible;
use std::future::{Future, Ready, ready};
use std::io::IoSlice;
use std::mem::size_of_val;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use http::header::CONTENT_TYPE;
use http::{HeaderName, HeaderValue, Response};
use http_body::Body as HttpBody;
use hyper::body::Incoming;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper::service::Service;

include!("response_template_renderers.rs");

const HEADER_FIELDS: [(&str, &str); 16] = [
    ("x-template-00", "value-00"),
    ("x-template-01", "value-01"),
    ("x-template-02", "value-02"),
    ("x-template-03", "value-03"),
    ("x-template-04", "value-04"),
    ("x-template-05", "value-05"),
    ("x-template-06", "value-06"),
    ("x-template-07", "value-07"),
    ("x-template-08", "value-08"),
    ("x-template-09", "value-09"),
    ("x-template-10", "value-10"),
    ("x-template-11", "value-11"),
    ("x-template-12", "value-12"),
    ("x-template-13", "value-13"),
    ("x-template-14", "value-14"),
    ("x-template-15", "value-15"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadScenario {
    Headers0,
    Headers1,
    Headers4,
    Headers16,
}

impl HeadScenario {
    const ALL: [Self; 4] = [Self::Headers0, Self::Headers1, Self::Headers4, Self::Headers16];

    const fn name(self) -> &'static str {
        match self {
            Self::Headers0 => "headers_0",
            Self::Headers1 => "headers_1",
            Self::Headers4 => "headers_4",
            Self::Headers16 => "headers_16",
        }
    }

    const fn count(self) -> usize {
        match self {
            Self::Headers0 => 0,
            Self::Headers1 => 1,
            Self::Headers4 => 4,
            Self::Headers16 => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadRepresentation {
    Ordinary,
    Reserved,
    StaticPlan,
    GeneratedPlan,
}

impl HeadRepresentation {
    const ALL: [Self; 4] = [Self::Ordinary, Self::Reserved, Self::StaticPlan, Self::GeneratedPlan];

    const fn name(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Reserved => "reserved",
            Self::StaticPlan => "static_plan",
            Self::GeneratedPlan => "generated_plan",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    length: usize,
    hash: u64,
}

impl Fingerprint {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn empty() -> Self {
        Self {
            length: 0,
            hash: Self::OFFSET,
        }
    }

    const fn of(bytes: &[u8]) -> Self {
        let mut hash = Self::OFFSET;
        let mut index = 0;
        while index < bytes.len() {
            hash ^= bytes[index] as u64;
            hash = hash.wrapping_mul(Self::PRIME);
            index += 1;
        }
        Self {
            length: bytes.len(),
            hash,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(Self::PRIME);
        }
        self.length += bytes.len();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HintObservation {
    lower: u64,
    upper: Option<u64>,
}

impl HintObservation {
    const EMPTY: Self = Self {
        lower: 0,
        upper: None,
    };

    fn of(hint: &http_body::SizeHint) -> Self {
        Self {
            lower: hint.lower(),
            upper: hint.upper(),
        }
    }

    const fn exact(length: u64) -> Self {
        Self {
            lower: length,
            upper: Some(length),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BodyObservation {
    status: u16,
    initial_end_stream: bool,
    final_end_stream: bool,
    polls: usize,
    frames: usize,
    frame_lengths: [usize; 3],
    hints: [HintObservation; 5],
    hint_count: usize,
    direct_static_bytes: usize,
    body: Fingerprint,
    body_size: usize,
}

fn run_body(representation: Representation, scenario: BodyScenario) -> BodyObservation {
    match representation {
        Representation::ExistingContiguous => observe_body(Response::new(existing_contiguous_body(scenario)), scenario),
        Representation::ExactContiguous => observe_body(Response::new(exact_contiguous_body(scenario)), scenario),
        Representation::Segmented => observe_body(Response::new(segmented_body(scenario)), scenario),
    }
}

#[expect(
    clippy::panic,
    reason = "a pending or failed in-memory response template is a benchmark invariant violation"
)]
fn observe_body<B>(response: Response<B>, scenario: BodyScenario) -> BodyObservation
where
    B: HttpBody<Data = Bytes>,
    B::Error: std::fmt::Debug,
{
    let status = response.status().as_u16();
    let body_size = size_of_val(response.body());
    let spans = scenario.static_spans();
    // Stack-pin to keep body polling allocation-free beyond construction.
    let mut body = std::pin::pin!(response.into_body());
    let initial_end_stream = body.as_ref().is_end_stream();
    let mut context = Context::from_waker(Waker::noop());
    let mut observation = BodyObservation {
        status,
        initial_end_stream,
        final_end_stream: initial_end_stream,
        polls: 0,
        frames: 0,
        frame_lengths: [0; 3],
        hints: [HintObservation::EMPTY; 5],
        hint_count: 1,
        direct_static_bytes: 0,
        body: Fingerprint::empty(),
        body_size,
    };
    observation.hints[0] = HintObservation::of(&body.as_ref().size_hint());

    loop {
        observation.polls += 1;
        match body.as_mut().poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => {
                let data = frame
                    .into_data()
                    .expect("generated response templates emit data frames only");
                let frame_index = observation.frames;
                observation.frames += 1;
                observation.frame_lengths[frame_index] = data.len();
                observation.direct_static_bytes += spans.overlap(data.as_ptr() as usize, data.len());
                observation.body.push(&data);
                observation.hints[observation.hint_count] = HintObservation::of(&body.as_ref().size_hint());
                observation.hint_count += 1;
            }
            Poll::Ready(Some(Err(error))) => panic!("response-template body failed: {error:?}"),
            Poll::Ready(None) => {
                observation.final_end_stream = body.as_ref().is_end_stream();
                observation.hints[observation.hint_count] = HintObservation::of(&body.as_ref().size_hint());
                observation.hint_count += 1;
                return observation;
            }
            Poll::Pending => panic!("response-template bodies are always ready"),
        }
    }
}

fn expected_hints(frame_lengths: [usize; 3], frame_count: usize) -> ([HintObservation; 5], usize) {
    let mut remaining = frame_lengths[..frame_count].iter().sum::<usize>() as u64;
    let mut hints = [HintObservation::EMPTY; 5];
    hints[0] = HintObservation::exact(remaining);
    for (index, frame_length) in frame_lengths[..frame_count].iter().enumerate() {
        remaining -= *frame_length as u64;
        hints[index + 1] = HintObservation::exact(remaining);
    }
    hints[frame_count + 1] = HintObservation::exact(remaining);
    (hints, frame_count + 2)
}

const HTTP_REQUEST: &[u8] =
    b"GET / HTTP/1.1\r\nHost: benchmark.invalid\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const TRANSPORT_CAPACITY: usize = 1024;

struct CaptureState {
    response: [u8; TRANSPORT_CAPACITY],
    response_length: usize,
    read_calls: usize,
    request_bytes: usize,
    write_calls: usize,
    vectored_write_calls: usize,
    io_slices: usize,
    flush_calls: usize,
    shutdown_calls: usize,
    direct_static_bytes: usize,
    spans: StaticSpans,
}

impl CaptureState {
    fn new(spans: StaticSpans) -> Self {
        Self {
            response: [0; TRANSPORT_CAPACITY],
            response_length: 0,
            read_calls: 0,
            request_bytes: 0,
            write_calls: 0,
            vectored_write_calls: 0,
            io_slices: 0,
            flush_calls: 0,
            shutdown_calls: 0,
            direct_static_bytes: 0,
            spans,
        }
    }

    fn record(&mut self, bytes: &[u8]) {
        let end = self
            .response_length
            .checked_add(bytes.len())
            .expect("the captured Hyper response length must fit in usize");
        assert!(end <= self.response.len(), "the captured Hyper response must fit in the fixed buffer");
        self.response[self.response_length..end].copy_from_slice(bytes);
        self.response_length = end;
        self.direct_static_bytes += self.spans.overlap(bytes.as_ptr() as usize, bytes.len());
    }
}

struct CaptureIo<'a> {
    state: &'a mut CaptureState,
    request_offset: usize,
}

impl Read for CaptureIo<'_> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        self.state.read_calls += 1;
        let remaining = &HTTP_REQUEST[self.request_offset..];
        if remaining.is_empty() && self.state.write_calls == 0 {
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let length = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..length]);
        self.request_offset += length;
        self.state.request_bytes += length;
        Poll::Ready(Ok(()))
    }
}

impl Write for CaptureIo<'_> {
    fn poll_write(mut self: Pin<&mut Self>, _context: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        self.state.write_calls += 1;
        self.state.io_slices += usize::from(!buf.is_empty());
        self.state.record(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.state.flush_calls += 1;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.state.shutdown_calls += 1;
        Poll::Ready(Ok(()))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.state.write_calls += 1;
        self.state.vectored_write_calls += 1;
        let mut written = 0;
        for buffer in bufs {
            if buffer.is_empty() {
                continue;
            }
            self.state.io_slices += 1;
            self.state.record(buffer);
            written += buffer.len();
        }
        Poll::Ready(Ok(written))
    }
}

struct OneResponse<B> {
    body: RefCell<Option<B>>,
}

impl<B> OneResponse<B> {
    fn new(body: B) -> Self {
        Self {
            body: RefCell::new(Some(body)),
        }
    }
}

impl<B> Service<http::Request<Incoming>> for OneResponse<B> {
    type Response = Response<B>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn call(&self, _request: http::Request<Incoming>) -> Self::Future {
        ready(Ok(Response::new(
            self.body
                .borrow_mut()
                .take()
                .expect("the one-response Hyper fixture receives exactly one request"),
        )))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TransportBodyCounters {
    polls: usize,
    frames: usize,
    size_hint_calls: usize,
    end_stream_calls: usize,
}

thread_local! {
    static TRANSPORT_BODY_COUNTERS: Cell<TransportBodyCounters> = const { Cell::new(TransportBodyCounters {
        polls: 0,
        frames: 0,
        size_hint_calls: 0,
        end_stream_calls: 0,
    }) };
}

struct TransportCounted<B>(B);

impl<B> HttpBody for TransportCounted<B>
where
    B: HttpBody<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        TRANSPORT_BODY_COUNTERS.set(TransportBodyCounters {
            polls: TRANSPORT_BODY_COUNTERS.get().polls + 1,
            ..TRANSPORT_BODY_COUNTERS.get()
        });
        let result = Pin::new(&mut self.0).poll_frame(cx);
        if matches!(result, Poll::Ready(Some(Ok(_)))) {
            TRANSPORT_BODY_COUNTERS.set(TransportBodyCounters {
                frames: TRANSPORT_BODY_COUNTERS.get().frames + 1,
                ..TRANSPORT_BODY_COUNTERS.get()
            });
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        TRANSPORT_BODY_COUNTERS.set(TransportBodyCounters {
            end_stream_calls: TRANSPORT_BODY_COUNTERS.get().end_stream_calls + 1,
            ..TRANSPORT_BODY_COUNTERS.get()
        });
        self.0.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        TRANSPORT_BODY_COUNTERS.set(TransportBodyCounters {
            size_hint_calls: TRANSPORT_BODY_COUNTERS.get().size_hint_calls + 1,
            ..TRANSPORT_BODY_COUNTERS.get()
        });
        self.0.size_hint()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransportObservation {
    status: u16,
    body: Fingerprint,
    body_size: usize,
    future_size: usize,
    connection_polls: usize,
    body_polls: usize,
    body_frames: usize,
    size_hint_calls: usize,
    end_stream_calls: usize,
    write_calls: usize,
    vectored_write_calls: usize,
    io_slices: usize,
    bytes_written: usize,
    direct_static_bytes: usize,
    copied_static_bytes: usize,
}

fn run_transport(representation: Representation, scenario: BodyScenario) -> TransportObservation {
    match representation {
        Representation::ExistingContiguous => transport_body(existing_contiguous_body(scenario), scenario),
        Representation::ExactContiguous => transport_body(exact_contiguous_body(scenario), scenario),
        Representation::Segmented => transport_body(segmented_body(scenario), scenario),
    }
}

#[expect(
    clippy::panic,
    reason = "a pending Hyper fixture after the bounded poll loop is a benchmark invariant violation"
)]
fn transport_body<B>(body: B, scenario: BodyScenario) -> TransportObservation
where
    B: HttpBody<Data = Bytes> + Unpin + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    TRANSPORT_BODY_COUNTERS.set(TransportBodyCounters::default());
    let body_size = size_of_val(&body);
    let mut capture = CaptureState::new(scenario.static_spans());
    let (future_size, connection_polls, connection_result) = {
        let io = CaptureIo {
            state: &mut capture,
            request_offset: 0,
        };
        let service = OneResponse::new(TransportCounted(body));
        let connection = hyper::server::conn::http1::Builder::new().serve_connection(io, service);
        let future_size = size_of_val(&connection);
        let mut connection = std::pin::pin!(connection);
        let mut context = Context::from_waker(Waker::noop());
        let mut polls = 0;
        let result = loop {
            polls += 1;
            match connection.as_mut().poll(&mut context) {
                Poll::Ready(result) => break result,
                Poll::Pending if polls < 64 => {}
                Poll::Pending => panic!("the in-memory Hyper response did not finish within 64 polls"),
            }
        };
        (future_size, polls, result)
    };
    connection_result.unwrap_or_else(|error| {
        panic!(
            "the in-memory Hyper response succeeds: {error:?}; reads={}, request_bytes={}, writes={}, response_bytes={}",
            capture.read_calls, capture.request_bytes, capture.write_calls, capture.response_length
        )
    });

    let response = &capture.response[..capture.response_length];
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .expect("Hyper emits a complete HTTP/1 response head");
    assert!(response.starts_with(b"HTTP/1.1 200 "), "Hyper emits a successful status line");
    let body = &response[header_end..];
    let counters = TRANSPORT_BODY_COUNTERS.get();
    let static_length = scenario.static_spans().total_length();
    TransportObservation {
        status: 200,
        body: Fingerprint::of(body),
        body_size,
        future_size,
        connection_polls,
        body_polls: counters.polls,
        body_frames: counters.frames,
        size_hint_calls: counters.size_hint_calls,
        end_stream_calls: counters.end_stream_calls,
        write_calls: capture.write_calls,
        vectored_write_calls: capture.vectored_write_calls,
        io_slices: capture.io_slices,
        bytes_written: capture.response_length,
        direct_static_bytes: capture.direct_static_bytes,
        copied_static_bytes: static_length.saturating_sub(capture.direct_static_bytes),
    }
}

fn prepare_transport(representation: Representation, scenario: BodyScenario) -> (Representation, BodyScenario) {
    std::hint::black_box(run_transport(representation, scenario));
    (representation, scenario)
}

fn run_prepared_transport(prepared: (Representation, BodyScenario)) -> TransportObservation {
    run_transport(prepared.0, prepared.1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeadObservation {
    status: u16,
    fields: usize,
    checksum: u64,
}

fn header_checksum(fields: &[(&str, &str)]) -> u64 {
    fields.iter().fold(0_u64, |checksum, (name, value)| {
        checksum
            .wrapping_add(Fingerprint::of(name.as_bytes()).hash)
            .wrapping_add(Fingerprint::of(value.as_bytes()).hash)
    })
}

const NEGOTIATED_CONTENT_TYPE: &str = "application/json";

fn run_head(scenario: HeadScenario) -> HeadObservation {
    run_head_with(HeadRepresentation::Ordinary, scenario, false)
}

fn run_head_with(representation: HeadRepresentation, scenario: HeadScenario, negotiated: bool) -> HeadObservation {
    let mut response = Response::new(Body::empty());
    match representation {
        HeadRepresentation::Ordinary => insert_ordinary(response.headers_mut(), scenario, negotiated),
        HeadRepresentation::Reserved => insert_reserved(response.headers_mut(), scenario, negotiated),
        HeadRepresentation::StaticPlan => insert_static_plan(response.headers_mut(), scenario, negotiated),
        HeadRepresentation::GeneratedPlan => insert_generated_plan(response.headers_mut(), scenario, negotiated),
    }
    let checksum = response.headers().iter().fold(0_u64, |checksum, (name, value)| {
        checksum
            .wrapping_add(Fingerprint::of(name.as_str().as_bytes()).hash)
            .wrapping_add(Fingerprint::of(value.as_bytes()).hash)
    });
    HeadObservation {
        status: response.status().as_u16(),
        fields: response.headers().len(),
        checksum,
    }
}

fn insert_ordinary(headers: &mut http::HeaderMap, scenario: HeadScenario, negotiated: bool) {
    for &(name, value) in &HEADER_FIELDS[..scenario.count()] {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    if negotiated {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(NEGOTIATED_CONTENT_TYPE));
    }
}

fn insert_reserved(headers: &mut http::HeaderMap, scenario: HeadScenario, negotiated: bool) {
    headers.reserve(scenario.count() + usize::from(negotiated));
    insert_ordinary(headers, scenario, negotiated);
}

macro_rules! extend_static_plan {
    ($headers:expr, $negotiated:expr, [$(($name:literal, $value:literal)),* $(,)?]) => {{
        if $negotiated {
            $headers.extend([
                $(
                    (
                        Some(const { HeaderName::from_static($name) }),
                        const { HeaderValue::from_static($value) },
                    ),
                )*
                (
                    Some(CONTENT_TYPE),
                    const { HeaderValue::from_static(NEGOTIATED_CONTENT_TYPE) },
                ),
            ]);
        } else {
            $headers.extend([
                $(
                    (
                        Some(const { HeaderName::from_static($name) }),
                        const { HeaderValue::from_static($value) },
                    ),
                )*
            ]);
        }
    }};
}

fn insert_static_plan(headers: &mut http::HeaderMap, scenario: HeadScenario, negotiated: bool) {
    match scenario {
        HeadScenario::Headers0 => {
            if negotiated {
                headers.extend([(
                    Some(CONTENT_TYPE),
                    const { HeaderValue::from_static(NEGOTIATED_CONTENT_TYPE) },
                )]);
            }
        }
        HeadScenario::Headers1 => {
            extend_static_plan!(headers, negotiated, [("x-template-00", "value-00")]);
        }
        HeadScenario::Headers4 => {
            extend_static_plan!(
                headers,
                negotiated,
                [
                    ("x-template-00", "value-00"),
                    ("x-template-01", "value-01"),
                    ("x-template-02", "value-02"),
                    ("x-template-03", "value-03"),
                ]
            );
        }
        HeadScenario::Headers16 => {
            extend_static_plan!(
                headers,
                negotiated,
                [
                    ("x-template-00", "value-00"),
                    ("x-template-01", "value-01"),
                    ("x-template-02", "value-02"),
                    ("x-template-03", "value-03"),
                    ("x-template-04", "value-04"),
                    ("x-template-05", "value-05"),
                    ("x-template-06", "value-06"),
                    ("x-template-07", "value-07"),
                    ("x-template-08", "value-08"),
                    ("x-template-09", "value-09"),
                    ("x-template-10", "value-10"),
                    ("x-template-11", "value-11"),
                    ("x-template-12", "value-12"),
                    ("x-template-13", "value-13"),
                    ("x-template-14", "value-14"),
                    ("x-template-15", "value-15"),
                ]
            );
        }
    }
}

macro_rules! insert_generated_fields {
    ($headers:expr, [$(($name:literal, $value:literal)),* $(,)?]) => {
        $(
            $headers.insert(
                const { HeaderName::from_static($name) },
                const { HeaderValue::from_static($value) },
            );
        )*
    };
}

fn insert_generated_plan(headers: &mut http::HeaderMap, scenario: HeadScenario, negotiated: bool) {
    match scenario {
        HeadScenario::Headers0 => {}
        HeadScenario::Headers1 => {
            insert_generated_fields!(headers, [("x-template-00", "value-00")]);
        }
        HeadScenario::Headers4 => {
            insert_generated_fields!(
                headers,
                [
                    ("x-template-00", "value-00"),
                    ("x-template-01", "value-01"),
                    ("x-template-02", "value-02"),
                    ("x-template-03", "value-03"),
                ]
            );
        }
        HeadScenario::Headers16 => {
            insert_generated_fields!(
                headers,
                [
                    ("x-template-00", "value-00"),
                    ("x-template-01", "value-01"),
                    ("x-template-02", "value-02"),
                    ("x-template-03", "value-03"),
                    ("x-template-04", "value-04"),
                    ("x-template-05", "value-05"),
                    ("x-template-06", "value-06"),
                    ("x-template-07", "value-07"),
                    ("x-template-08", "value-08"),
                    ("x-template-09", "value-09"),
                    ("x-template-10", "value-10"),
                    ("x-template-11", "value-11"),
                    ("x-template-12", "value-12"),
                    ("x-template-13", "value-13"),
                    ("x-template-14", "value-14"),
                    ("x-template-15", "value-15"),
                ]
            );
        }
    }
    if negotiated {
        headers.insert(
            CONTENT_TYPE,
            const { HeaderValue::from_static(NEGOTIATED_CONTENT_TYPE) },
        );
    }
}

fn assert_equivalent() {
    for representation in Representation::ALL {
        for scenario in BodyScenario::ALL {
            let observation = run_body(representation, scenario);
            let (frame_lengths, frame_count) = expected_frame_lengths(representation, scenario);
            let (hints, hint_count) = expected_hints(frame_lengths, frame_count);
            let static_length = scenario.static_spans().total_length();
            assert_eq!(observation.status, 200);
            assert!(!observation.initial_end_stream);
            assert!(observation.final_end_stream);
            assert_eq!(observation.polls, frame_count + 1);
            assert_eq!(observation.frames, frame_count);
            assert_eq!(observation.frame_lengths, frame_lengths);
            assert_eq!(observation.hints, hints);
            assert_eq!(observation.hint_count, hint_count);
            assert_eq!(
                static_length - observation.direct_static_bytes,
                expected_copied_static_bytes(representation, scenario),
                "{} / {} changed its in-memory static-copy contract",
                representation.name(),
                scenario.name()
            );
            assert_eq!(
                observation.body,
                Fingerprint::of(scenario.expected()),
                "{} / {} changed its response bytes",
                representation.name(),
                scenario.name()
            );

            let transport = run_transport(representation, scenario);
            assert_eq!(transport.status, 200);
            assert_eq!(
                transport.body,
                Fingerprint::of(scenario.expected()),
                "{} / {} changed its Hyper HTTP/1 response bytes",
                representation.name(),
                scenario.name()
            );
            assert_eq!(transport.body_frames, frame_count);
            assert!(transport.body_polls >= transport.body_frames);
            assert!(transport.size_hint_calls > 0);
            assert_eq!(
                transport.direct_static_bytes + transport.copied_static_bytes,
                static_length
            );
        }
    }

    for representation in HeadRepresentation::ALL {
        for scenario in HeadScenario::ALL {
            for negotiated in [false, true] {
                let negotiated_checksum = if negotiated {
                    header_checksum(&[("content-type", NEGOTIATED_CONTENT_TYPE)])
                } else {
                    0
                };
                assert_eq!(
                    run_head_with(representation, scenario, negotiated),
                    HeadObservation {
                        status: 200,
                        fields: scenario.count() + usize::from(negotiated),
                        checksum: header_checksum(&HEADER_FIELDS[..scenario.count()]).wrapping_add(negotiated_checksum),
                    },
                    "response head {} / {} / negotiated={negotiated} changed its inserted fields",
                    representation.name(),
                    scenario.name()
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

fn measure_allocations(operation_name: &str, operation: impl FnOnce()) -> AllocationStats {
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let measured = session.operation(operation_name);
    {
        let _span = measured.measure_thread().iterations(1);
        operation();
    }
    let report = session.to_report();
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == operation_name)
        .expect("the response-template allocation operation is recorded");
    AllocationStats {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
    }
}

fn body_allocation_diagnostics() -> [[(BodyScenario, AllocationStats); 4]; 3] {
    Representation::ALL.map(|representation| {
        BodyScenario::ALL.map(|scenario| {
            (
                scenario,
                measure_allocations("construct_and_observe", || {
                    std::hint::black_box(run_body(representation, scenario));
                }),
            )
        })
    })
}

fn transport_allocation_diagnostics() -> [[(BodyScenario, AllocationStats); 4]; 3] {
    Representation::ALL.map(|representation| {
        BodyScenario::ALL.map(|scenario| {
            (
                scenario,
                measure_allocations("render_and_http1", || {
                    std::hint::black_box(run_transport(representation, scenario));
                }),
            )
        })
    })
}

fn head_allocation_diagnostics() -> [(HeadScenario, AllocationStats); 4] {
    HeadScenario::ALL.map(|scenario| {
        (
            scenario,
            measure_allocations("insert", || {
                std::hint::black_box(run_head(scenario));
            }),
        )
    })
}

fn head_candidate_allocation_diagnostics() -> [[[(HeadScenario, AllocationStats); 4]; 2]; 4] {
    HeadRepresentation::ALL.map(|representation| {
        [false, true].map(|negotiated| {
            HeadScenario::ALL.map(|scenario| {
                (
                    scenario,
                    measure_allocations("insert_candidate", || {
                        std::hint::black_box(run_head_with(representation, scenario, negotiated));
                    }),
                )
            })
        })
    })
}
