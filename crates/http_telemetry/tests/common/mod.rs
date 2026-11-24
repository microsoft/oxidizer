// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Processors, sinks, requests, handlers and bodies that the integration tests and the benchmark
//! share.

#![allow(dead_code, reason = "each test binary and the benchmark use a different part of this module")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::borrow::Cow;
use std::collections::VecDeque;
use std::future::{pending, ready};
use std::hint::black_box;
use std::io::{self, ErrorKind};
use std::ops::ControlFlow;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bytesbuf::BytesView;
use data_privacy::{DataClass, RedactionEngine, Sensitive};
use http::header::RETRY_AFTER;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri, Version};
use http_body::{Body, Frame, SizeHint};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder, HttpResponse, RequestHandler};
use http_telemetry::{Client, Server, Telemetry, TelemetryLayer};
use layered::{Execute, Service};
use observed::metadata::{EventDescription, FieldDescriptor, InstrumentKind};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{Enrichment, FlushError, Severity, Sink, SinkId, Value};
use templated_uri::{BaseUri, EscapedString, templated};
use tick::{ClockControl, SimpleClock};

pub(crate) const COMPLETE: &str = "http.server.request.complete";
pub(crate) const ERROR: &str = "http.server.request.error";
pub(crate) const ABANDONED: &str = "http.server.request.abandoned";
pub(crate) const ACTIVE: &str = "http.server.active_requests";
pub(crate) const DURATION: &str = "http.server.request.duration";
pub(crate) const CLIENT_COMPLETE: &str = "http.client.request.complete";
pub(crate) const CLIENT_ERROR: &str = "http.client.request.error";
pub(crate) const CLIENT_ABANDONED: &str = "http.client.request.abandoned";
pub(crate) const CLIENT_DURATION: &str = "http.client.request.duration";
pub(crate) const TOTAL_COMPLETE: &str = "http.client.request.total.complete";
pub(crate) const TOTAL_ERROR: &str = "http.client.request.total.error";
pub(crate) const TOTAL_ABANDONED: &str = "http.client.request.total.abandoned";
pub(crate) const TOTAL_DURATION: &str = "http.client.request.total_duration";

/// A data class whose values [`public_engine`] prints unchanged.
pub(crate) const PUBLIC: DataClass = DataClass::new("test", "public");

/// A data class whose values [`public_engine`] erases.
pub(crate) const SECRET: DataClass = DataClass::new("test", "secret");

/// A redaction engine that prints the values of [`PUBLIC`] unchanged, and erases the values of every other class.
pub(crate) fn public_engine() -> RedactionEngine {
    RedactionEngine::builder().suppress_redaction(PUBLIC).build()
}

/// A server layer over `sink` with a frozen clock. If a test does not set its own clock, each
/// duration is `0`.
pub(crate) fn server(sink: Sink) -> TelemetryLayer<Server> {
    Telemetry::server(sink).clock(SimpleClock::new_frozen())
}

/// A client layer over `sink` with a frozen clock. If a test does not set its own clock, each
/// duration is `0`.
pub(crate) fn client(sink: Sink) -> TelemetryLayer<Client> {
    Telemetry::client(sink).clock(SimpleClock::new_frozen())
}

/// The bucket boundaries of each duration metric: the boundaries that the OpenTelemetry HTTP metrics
/// conventions give for request durations, in seconds.
pub(crate) const DURATION_BOUNDARIES: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0];

/// The value that one field records in a metric, and the description of that metric.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Measurement {
    pub(crate) instrument: &'static str,
    pub(crate) kind: InstrumentKind,
    pub(crate) description: &'static str,
    pub(crate) unit: &'static str,
    pub(crate) boundaries: &'static [f64],
    pub(crate) value: Value,
}

/// The value of the duration metric for `value` seconds.
pub(crate) fn duration(value: f64) -> Measurement {
    Measurement {
        instrument: DURATION,
        kind: InstrumentKind::Histogram,
        description: "Duration of HTTP server requests.",
        unit: "s",
        boundaries: DURATION_BOUNDARIES,
        value: Value::F64(value),
    }
}

/// The value of the metric `http.client.request.duration` for `value` seconds.
pub(crate) fn client_duration(value: f64) -> Measurement {
    Measurement {
        instrument: CLIENT_DURATION,
        kind: InstrumentKind::Histogram,
        description: "Duration of HTTP client requests.",
        unit: "s",
        boundaries: DURATION_BOUNDARIES,
        value: Value::F64(value),
    }
}

/// The value of the metric `http.client.request.total_duration` for `value` seconds.
pub(crate) fn total_duration(value: f64) -> Measurement {
    Measurement {
        instrument: TOTAL_DURATION,
        kind: InstrumentKind::Histogram,
        description: "Total duration of HTTP client requests, including all retries and hedged attempts.",
        unit: "s",
        boundaries: DURATION_BOUNDARIES,
        value: Value::F64(value),
    }
}

/// One event, as a processor saw it. Two events are equal if they have the same fields in any order, because
/// `observed` does not promise an order of the fields.
#[derive(Debug, Clone)]
pub(crate) struct Captured {
    pub(crate) name: &'static str,
    pub(crate) severity: Option<Severity>,
    pub(crate) message: Option<Cow<'static, str>>,
    /// The log fields, under their log keys.
    pub(crate) logs: Vec<(&'static str, Value)>,
    /// The fields that are metric dimensions, under their metric keys.
    pub(crate) dimensions: Vec<(&'static str, Value)>,
    /// The fields that give the value of a metric, such as the duration.
    pub(crate) measurements: Vec<Measurement>,
    /// The enrichment entries, under their keys.
    pub(crate) enrichments: Vec<(&'static str, Value)>,
}

impl PartialEq for Captured {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            name,
            severity,
            message,
            logs,
            dimensions,
            measurements,
            enrichments,
        } = self;
        *name == other.name
            && *severity == other.severity
            && *message == other.message
            && sorted(logs) == sorted(&other.logs)
            && sorted(dimensions) == sorted(&other.dimensions)
            && *measurements == other.measurements
            && sorted(enrichments) == sorted(&other.enrichments)
    }
}

/// `fields` sorted by key, so that a comparison does not depend on the order of the fields.
pub(crate) fn sorted<'a>(fields: &'a [(&'static str, Value)]) -> Vec<&'a (&'static str, Value)> {
    let mut sorted: Vec<_> = fields.iter().collect();
    sorted.sort_by_key(|(key, _)| *key);
    sorted
}

impl Captured {
    /// The value of the log field `key`.
    pub(crate) fn log(&self, key: &str) -> Option<&Value> {
        self.logs.iter().find_map(|(log_key, value)| (*log_key == key).then_some(value))
    }

    /// The value of the metric dimension `key`.
    pub(crate) fn dimension(&self, key: &str) -> Option<&Value> {
        self.dimensions
            .iter()
            .find_map(|(dimension_key, value)| (*dimension_key == key).then_some(value))
    }
}

/// Captures each event whose name it is interested in. It keeps log fields, metric dimensions,
/// metric values and enrichments apart, so a test can tell a log-only field from a metric
/// dimension.
#[derive(Clone)]
pub(crate) struct Capture {
    interested: fn(&str) -> bool,
    engine: RedactionEngine,
    events: Arc<Mutex<Vec<Captured>>>,
}

impl Capture {
    /// Captures each event whose name `interested` accepts. It prints each classified value
    /// unchanged.
    pub(crate) fn new(interested: fn(&str) -> bool) -> Self {
        Self::with_engine(interested, observed_testing::passthrough_redaction_engine())
    }

    /// Captures each event whose name `interested` accepts, and redacts each classified value with
    /// `engine`.
    pub(crate) fn with_engine(interested: fn(&str) -> bool, engine: RedactionEngine) -> Self {
        Self {
            interested,
            engine,
            events: Arc::default(),
        }
    }

    pub(crate) fn events(&self) -> Vec<Captured> {
        self.events.lock().unwrap().clone()
    }

    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.events.lock().unwrap().iter().map(|event| event.name).collect()
    }

    pub(crate) fn single(&self) -> Captured {
        let events = self.events();
        assert_eq!(events.len(), 1, "the processor must get exactly one event: {events:#?}");
        events.into_iter().next().unwrap()
    }
}

impl EventProcessor for Capture {
    fn is_interested(&self, description: &EventDescription) -> bool {
        (self.interested)(description.name())
    }

    fn process(&self, event: &EventView<'_>) {
        let engine = &self.engine;
        let mut captured = Captured {
            name: event.name(),
            severity: event.severity(),
            message: event.body(),
            logs: Vec::new(),
            dimensions: Vec::new(),
            measurements: Vec::new(),
            enrichments: Vec::new(),
        };
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            let value = value(engine);
            if let Some(log) = descriptor.log() {
                captured.logs.push((log.key(), value.clone()));
            }
            if let Some(metric) = descriptor.metric() {
                match metric.instrument_description() {
                    Some(instrument) => captured.measurements.push(Measurement {
                        instrument: instrument.instrument_name(),
                        kind: instrument.kind(),
                        description: instrument.description(),
                        unit: instrument.unit(),
                        boundaries: instrument.boundaries(),
                        value,
                    }),
                    None => captured.dimensions.push((metric.key(), value)),
                }
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            captured.enrichments.push((descriptor.field_name(), value(engine)));
            ControlFlow::Continue(())
        });
        self.events.lock().unwrap().push(captured);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink whose one processor is `capture`.
pub(crate) fn sink_with(capture: &Capture) -> Sink {
    Sink::builder(SinkId::new("capture"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(capture.clone())])
        .build()
}

/// A sink that captures every event except the deltas of `http.server.active_requests`.
pub(crate) fn capture_sink() -> (Sink, Capture) {
    let capture = Capture::new(|name| name != ACTIVE);
    (sink_with(&capture), capture)
}

/// Reads each field that `reads` accepts and every enrichment of each event whose name `interested`
/// accepts, and keeps nothing. Thus the measurements do not include the work of keeping the events.
pub(crate) struct ReadAll {
    pub(crate) engine: RedactionEngine,
    pub(crate) interested: fn(&str) -> bool,
    pub(crate) reads: fn(&FieldDescriptor) -> bool,
}

impl EventProcessor for ReadAll {
    fn is_interested(&self, description: &EventDescription) -> bool {
        (self.interested)(description.name())
    }

    fn process(&self, event: &EventView<'_>) {
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            if (self.reads)(descriptor) {
                black_box(value(&self.engine));
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |_: &FieldDescriptor, value: &FieldValueFn<'_>| {
            black_box(value(&self.engine));
            ControlFlow::Continue(())
        });
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink with `count` processors that each read every field of every event. Their engine prints
/// the values of [`PUBLIC`] unchanged, and erases the values of every other data class.
pub(crate) fn reading_sink(count: usize) -> Sink {
    Sink::builder(SinkId::new("reading"))
        .clock(SimpleClock::new_frozen())
        .processors(
            (0..count)
                .map(|_| {
                    Arc::new(ReadAll {
                        engine: public_engine(),
                        interested: |_| true,
                        reads: |_| true,
                    }) as Arc<dyn EventProcessor>
                })
                .collect(),
        )
        .build()
}

/// Receives every event and reads none of its fields.
pub(crate) struct Interested;

impl EventProcessor for Interested {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, _: &EventView<'_>) {}

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink whose one processor receives every event and reads none of its fields.
pub(crate) fn interested_sink() -> Sink {
    Sink::builder(SinkId::new("interested"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(Interested)])
        .build()
}

/// Tells the sink whether it is interested, with an answer that the test can change. It records
/// the name of each event that it receives.
#[derive(Clone, Default)]
pub(crate) struct Probe {
    interested: Arc<AtomicBool>,
    received: Arc<Mutex<Vec<&'static str>>>,
}

impl Probe {
    pub(crate) fn set_interested(&self, interested: bool) {
        self.interested.store(interested, Ordering::SeqCst);
    }

    /// The names of the events that the processor received.
    pub(crate) fn received(&self) -> Vec<&'static str> {
        self.received.lock().unwrap().clone()
    }
}

impl EventProcessor for Probe {
    fn is_interested(&self, _: &EventDescription) -> bool {
        self.interested.load(Ordering::SeqCst)
    }

    fn process(&self, event: &EventView<'_>) {
        self.received.lock().unwrap().push(event.name());
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink whose one processor is `probe`.
pub(crate) fn probe_sink(probe: &Probe) -> Sink {
    Sink::builder(SinkId::new("probe"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(probe.clone())])
        .build()
}

/// Stops the visit of the fields of `http.server.request.complete` at the first field whose log key starts with a
/// prefix. It records the log key of each field that the visit still reaches after that field.
#[derive(Clone)]
pub(crate) struct StopAt {
    prefix: &'static str,
    after_stop: Arc<Mutex<Option<Vec<&'static str>>>>,
}

impl StopAt {
    pub(crate) fn new(prefix: &'static str) -> Self {
        Self {
            prefix,
            after_stop: Arc::default(),
        }
    }

    /// The log keys of the fields that the visit reached after the stop, or `None` if the visit did not stop.
    pub(crate) fn after_stop(&self) -> Option<Vec<&'static str>> {
        self.after_stop.lock().unwrap().clone()
    }
}

impl EventProcessor for StopAt {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == COMPLETE
    }

    fn process(&self, event: &EventView<'_>) {
        let mut after_stop = self.after_stop.lock().unwrap();
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, _: &FieldValueFn<'_>| {
            let key = descriptor.log().map_or_else(|| descriptor.field_name(), |log| log.key());
            if let Some(after) = after_stop.as_mut() {
                after.push(key);
            } else if key.starts_with(self.prefix) {
                *after_stop = Some(Vec::new());
            } else {
                return ControlFlow::Continue(());
            }
            ControlFlow::Break(())
        });
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink whose one processor is `stop`.
pub(crate) fn stop_sink(stop: &StopAt) -> Sink {
    Sink::builder(SinkId::new("stop"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(stop.clone())])
        .build()
}

/// Sends `request` to `handler`, which responds or returns an error at the first poll.
pub(crate) fn send(handler: &impl RequestHandler, request: HttpRequest) -> Result<HttpResponse, HttpError> {
    // Pin on the stack, because `Box::pin` allocates and so changes the measurement.
    let future = pin!(handler.execute(request));
    let Poll::Ready(result) = future.poll(&mut Context::from_waker(Waker::noop())) else {
        unreachable!("the handler finishes at the first poll, and the layer does not wait");
    };
    result
}

/// Sends `request` to `handler`, which never responds. Then polls the future once and drops it.
pub(crate) fn abandon(handler: &impl RequestHandler, request: HttpRequest) {
    // Pin on the stack, because `Box::pin` allocates and so changes the measurement.
    let future = pin!(handler.execute(request));
    assert!(
        future.poll(&mut Context::from_waker(Waker::noop())).is_pending(),
        "the handler never responds"
    );
}

/// A request with `method` and `version`, and an empty body.
pub(crate) fn request(method: Method, version: Version) -> HttpRequest {
    Request::builder()
        .method(method)
        .version(version)
        .uri("/books/1")
        .body(HttpBodyBuilder::new_fake().empty())
        .unwrap()
}

/// A `GET` request over HTTP/1.1.
pub(crate) fn get() -> HttpRequest {
    request(Method::GET, Version::HTTP_11)
}

/// A `GET` request over HTTP/1.1 to `uri`. The URI keeps the static text, so a clone of its
/// authority does not allocate.
pub(crate) fn get_to(uri: &'static str) -> HttpRequest {
    Request::get(Uri::from_static(uri))
        .body(HttpBodyBuilder::new_fake().empty())
        .unwrap()
}

/// A `PURGE` request, which `http.request.method` records as `_OTHER`.
pub(crate) fn purge() -> HttpRequest {
    with_method(b"PURGE")
}

/// A request with a method that `http` has no constant for, and that is longer than 15 bytes.
/// `http` keeps such a method on the heap.
pub(crate) fn long_extension_method() -> HttpRequest {
    with_method(b"LONG-EXTENSION-METHOD")
}

/// A request over HTTP/1.1 to `/books/1` with `method`. The URI keeps the static text, so a clone of
/// its path does not allocate.
fn with_method(method: &[u8]) -> HttpRequest {
    let mut request = get_to("/books/1");
    *request.method_mut() = Method::from_bytes(method).unwrap();
    request
}

/// The path of one book. The book has the data class [`SECRET`].
#[templated(template = "/books/{book}")]
#[derive(Clone)]
pub(crate) struct BookPath {
    pub(crate) book: Sensitive<EscapedString>,
}

/// A `GET` request over HTTP/1.1 to `https://example.com/books/1`, built from [`BookPath`] with
/// `HttpRequestBuilder`, so the request has the template of its path.
pub(crate) fn get_book() -> HttpRequest {
    let uri = templated_uri::Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path_and_query(BookPath {
            book: Sensitive::new(EscapedString::from_static("1"), SECRET),
        });
    HttpRequestBuilder::new_fake().get(uri).build().unwrap()
}

/// The header that the header tests and benchmarks log for each request.
pub(crate) const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// A request for `http://example.com/books/1` with the header `x-request-id`, and with `retry_after` as the header
/// `retry-after`. [`Echoing`] copies both headers to the response.
pub(crate) fn with_headers(retry_after: HeaderValue) -> HttpRequest {
    let mut request = get_to("http://example.com/books/1");
    request.headers_mut().insert(REQUEST_ID, HeaderValue::from_static("7"));
    request.headers_mut().insert(RETRY_AFTER, retry_after);
    request
}

/// An enrichment with the metric dimension `tenant`.
#[derive(Enrichment)]
pub(crate) struct Tenant {
    #[dimension(metric = "tenant")]
    #[unredacted]
    pub(crate) tenant: &'static str,
}

/// Responds to each request with its status and an empty body. It does not allocate.
#[derive(Debug)]
pub(crate) struct Responding {
    status: StatusCode,
    bodies: HttpBodyBuilder,
}

impl Service<HttpRequest> for Responding {
    type Out = http_extensions::Result<HttpResponse>;

    fn execute(&self, _request: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let mut response = HttpResponse::new(self.bodies.empty());
        *response.status_mut() = self.status;
        ready(Ok(response))
    }
}

/// A handler that responds to each request with `status` and an empty body.
pub(crate) fn responding(status: StatusCode) -> Responding {
    Responding {
        status,
        bodies: HttpBodyBuilder::new_fake(),
    }
}

/// A handler that moves `clock` forward by `elapsed` for each request, and then responds with
/// `status` and an empty body.
pub(crate) fn responding_after(clock: &ClockControl, elapsed: Duration, status: StatusCode) -> impl RequestHandler {
    let clock = clock.clone();
    let bodies = HttpBodyBuilder::new_fake();
    Execute::new(move |_: HttpRequest| {
        clock.advance(elapsed);
        let mut response = HttpResponse::new(bodies.empty());
        *response.status_mut() = status;
        async move { Ok::<_, HttpError>(response) }
    })
}

/// Responds to each request with its status, and with the version, the headers and the body of the
/// request. It does not allocate, so a benchmark can build the response body before the measurement.
pub(crate) struct Echoing(pub(crate) StatusCode);

impl Service<HttpRequest> for Echoing {
    type Out = http_extensions::Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let (parts, body) = input.into_parts();
        let mut response = HttpResponse::new(body);
        *response.status_mut() = self.0;
        *response.version_mut() = parts.version;
        *response.headers_mut() = parts.headers;
        ready(Ok(response))
    }
}

/// One poll of a [`Scripted`] body.
#[expect(
    clippy::large_enum_variant,
    reason = "BytesView is intentionally large, though future optimizations may decrease size"
)]
pub(crate) enum Step {
    /// Returns a data frame with these bytes.
    Data(BytesView),
    /// Returns a trailers frame without fields.
    Trailers,
    /// Returns `Poll::Pending`.
    Pending,
    /// Returns this error.
    Error(HttpError),
    /// Panics.
    Panic,
    /// Returns `None`.
    End,
}

/// A data step with the bytes of `text`.
pub(crate) fn data(text: &str) -> Step {
    Step::Data(BytesView::copied_from_slice(text.as_bytes(), &HttpBodyBuilder::new_fake()))
}

/// A body that takes one step for each poll, and returns `None` after the last step. It is at its
/// end when no step is left. A sized body has an exact size hint: the length of its data steps that
/// are left. The length of other bodies is unknown.
pub(crate) struct Scripted {
    steps: VecDeque<Step>,
    sized: bool,
}

impl Body for Scripted {
    type Data = BytesView;
    type Error = HttpError;

    #[expect(clippy::panic, reason = "the step `Panic` makes the body panic")]
    fn poll_frame(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<http_extensions::Result<Frame<BytesView>>>> {
        match self.steps.pop_front() {
            Some(Step::Data(data)) => Poll::Ready(Some(Ok(Frame::data(data)))),
            Some(Step::Trailers) => Poll::Ready(Some(Ok(Frame::trailers(HeaderMap::new())))),
            Some(Step::Pending) => Poll::Pending,
            Some(Step::Error(error)) => Poll::Ready(Some(Err(error))),
            Some(Step::Panic) => panic!("the body panicked"),
            Some(Step::End) | None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.steps.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        if !self.sized {
            return SizeHint::default();
        }
        let length: usize = self
            .steps
            .iter()
            .map(|step| if let Step::Data(data) = step { data.len() } else { 0 })
            .sum();
        SizeHint::with_exact(u64::try_from(length).unwrap())
    }
}

/// A streamed body that takes `steps`. Its length is unknown.
pub(crate) fn scripted(steps: impl IntoIterator<Item = Step>) -> HttpBody {
    let steps = steps.into_iter().collect();
    HttpBodyBuilder::new_fake().body(Scripted { steps, sized: false })
}

/// A streamed body that takes `steps`, with an exact size hint.
pub(crate) fn sized_scripted(steps: impl IntoIterator<Item = Step>) -> HttpBody {
    let steps = steps.into_iter().collect();
    HttpBodyBuilder::new_fake().body(Scripted { steps, sized: true })
}

/// Polls `body` once, with a waker that does nothing.
pub(crate) fn poll_frame(body: &mut HttpBody) -> Poll<Option<http_extensions::Result<Frame<BytesView>>>> {
    Pin::new(body).poll_frame(&mut Context::from_waker(Waker::noop()))
}

/// A handler that never responds.
pub(crate) struct NeverResponding;

impl Service<HttpRequest> for NeverResponding {
    type Out = http_extensions::Result<HttpResponse>;

    fn execute(&self, _request: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        pending()
    }
}

/// Returns the error that its function makes, a new one for each request.
pub(crate) struct Failing(fn() -> HttpError);

impl Service<HttpRequest> for Failing {
    type Out = http_extensions::Result<HttpResponse>;

    fn execute(&self, _request: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        ready(Err((self.0)()))
    }
}

/// A handler that returns `HttpError::validation`, an error without a source.
pub(crate) const FAILING: Failing = Failing(|| HttpError::validation("the review has no text"));

/// A handler that returns an `HttpError` around a refused connection.
pub(crate) const FAILING_CHAINED: Failing = Failing(|| HttpError::from(io::Error::from(ErrorKind::ConnectionRefused)));
