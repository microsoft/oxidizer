// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Processors, sinks, requests, handlers and bodies that the integration tests and the benchmark
//! share.

#![allow(dead_code, reason = "each test binary and the benchmark use a different part of this module")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::borrow::Cow;
use std::collections::VecDeque;
use std::future::{pending, ready};
use std::io::{self, ErrorKind};
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, SystemTime};

use bytesbuf::BytesView;
use http::{HeaderMap, Method, Request, StatusCode, Version};
use http_body::{Body, Frame};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpBodyOptions, HttpError, HttpRequest, HttpResponse, RequestHandler};
use http_telemetry::{Telemetry, TelemetryLayer};
use layered::{Execute, Service};
use observed::metadata::{EventDescription, FieldDescriptor, InstrumentKind};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{Enrichment, FlushError, Severity, Sink, SinkId, Value};
use tick::{ClockControl, SimpleClock};

pub(crate) const COMPLETE: &str = "http.server.request.complete";
pub(crate) const ERROR: &str = "http.server.request.error";
pub(crate) const ABANDONED: &str = "http.server.request.abandoned";
pub(crate) const ACTIVE: &str = "http.server.active_requests";
pub(crate) const DURATION: &str = "http.server.request.duration";

/// A server layer over `sink` with a frozen clock. If a test does not set its own clock, each
/// duration is `0`.
pub(crate) fn server(sink: Sink) -> TelemetryLayer {
    Telemetry::server(sink).clock(SimpleClock::new_frozen())
}

/// The time of the frozen clock of [`capture_sink`].
pub(crate) fn sink_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
}

/// The value that one field records in a metric, and the description of that metric.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Measurement {
    pub(crate) instrument: &'static str,
    pub(crate) kind: InstrumentKind,
    pub(crate) description: &'static str,
    pub(crate) unit: &'static str,
    pub(crate) value: Value,
}

/// The value of the duration metric for `value` milliseconds.
pub(crate) fn duration(value: u64) -> Measurement {
    Measurement {
        instrument: DURATION,
        kind: InstrumentKind::Histogram,
        description: "Duration of HTTP server requests.",
        unit: "ms",
        value: Value::U64(value),
    }
}

/// One event, as a processor saw it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Captured {
    pub(crate) name: &'static str,
    pub(crate) severity: Option<Severity>,
    pub(crate) message: Option<Cow<'static, str>>,
    pub(crate) timestamp: SystemTime,
    /// The log fields, under their log keys.
    pub(crate) logs: Vec<(&'static str, Value)>,
    /// The fields that are metric dimensions, under their metric keys.
    pub(crate) dimensions: Vec<(&'static str, Value)>,
    /// The fields that give the value of a metric, such as the duration.
    pub(crate) measurements: Vec<Measurement>,
    /// The enrichment entries, under their keys.
    pub(crate) enrichments: Vec<(&'static str, Value)>,
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
    events: Arc<Mutex<Vec<Captured>>>,
}

impl Capture {
    /// Captures each event whose name `interested` accepts.
    pub(crate) fn new(interested: fn(&str) -> bool) -> Self {
        Self {
            interested,
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
        let engine = observed_testing::passthrough_redaction_engine();
        let mut captured = Captured {
            name: event.name(),
            severity: event.severity(),
            message: event.body(),
            timestamp: event.timestamp(),
            logs: Vec::new(),
            dimensions: Vec::new(),
            measurements: Vec::new(),
            enrichments: Vec::new(),
        };
        let _ = event.visit_fields(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            let value = value(&engine);
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
                        value,
                    }),
                    None => captured.dimensions.push((metric.key(), value)),
                }
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |descriptor: &FieldDescriptor, value: &FieldValueFn<'_>| {
            captured.enrichments.push((descriptor.field_name(), value(&engine)));
            ControlFlow::Continue(())
        });
        self.events.lock().unwrap().push(captured);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// A sink whose one processor is `capture`, and a frozen clock at [`sink_time`].
pub(crate) fn sink_with(capture: &Capture) -> Sink {
    Sink::builder(SinkId::new("capture"))
        .clock(SimpleClock::new_frozen_at(sink_time()))
        .processors(vec![Arc::new(capture.clone())])
        .build()
}

/// A sink that captures every event except the deltas of `http.server.active_requests`.
pub(crate) fn capture_sink() -> (Sink, Capture) {
    let capture = Capture::new(|name| name != ACTIVE);
    (sink_with(&capture), capture)
}

/// Tells the sink whether it is interested, with an answer that the test can change. It records
/// each event description that the sink asks about, and the name of each event that it receives.
#[derive(Clone, Default)]
pub(crate) struct Probe {
    interested: Arc<AtomicBool>,
    asked: Arc<Mutex<Vec<EventDescription>>>,
    received: Arc<Mutex<Vec<&'static str>>>,
}

impl Probe {
    pub(crate) fn set_interested(&self, interested: bool) {
        self.interested.store(interested, Ordering::SeqCst);
    }

    /// The event descriptions that the sink asked about.
    pub(crate) fn asked(&self) -> Vec<EventDescription> {
        self.asked.lock().unwrap().clone()
    }

    /// The names of the events that the processor received.
    pub(crate) fn received(&self) -> Vec<&'static str> {
        self.received.lock().unwrap().clone()
    }
}

impl EventProcessor for Probe {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.asked.lock().unwrap().push(*description);
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

/// A `GET` request over HTTP/1.1 to `uri`.
pub(crate) fn get_to(uri: &'static str) -> HttpRequest {
    Request::get(uri).body(HttpBodyBuilder::new_fake().empty()).unwrap()
}

/// An enrichment with the metric dimension `tenant`.
#[derive(Enrichment)]
pub(crate) struct Tenant {
    #[dimension(metric = "tenant")]
    #[unredacted]
    pub(crate) tenant: &'static str,
}

/// Responds to each request with its status and an empty body. It does not allocate.
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

/// Responds to each request with its status, and with the headers and the body of the request. It
/// does not allocate, so a benchmark can build the response body before the measurement.
pub(crate) struct Echoing(pub(crate) StatusCode);

impl Service<HttpRequest> for Echoing {
    type Out = http_extensions::Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        let (parts, body) = input.into_parts();
        let mut response = HttpResponse::new(body);
        *response.status_mut() = self.0;
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
/// end when no step is left. Its length is unknown.
pub(crate) struct Scripted(VecDeque<Step>);

impl Body for Scripted {
    type Data = BytesView;
    type Error = HttpError;

    #[expect(clippy::panic, reason = "the step `Panic` makes the body panic")]
    fn poll_frame(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<http_extensions::Result<Frame<BytesView>>>> {
        match self.0.pop_front() {
            Some(Step::Data(data)) => Poll::Ready(Some(Ok(Frame::data(data)))),
            Some(Step::Trailers) => Poll::Ready(Some(Ok(Frame::trailers(HeaderMap::new())))),
            Some(Step::Pending) => Poll::Pending,
            Some(Step::Error(error)) => Poll::Ready(Some(Err(error))),
            Some(Step::Panic) => panic!("the body panicked"),
            Some(Step::End) | None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_empty()
    }
}

/// A streamed body that takes `steps`.
pub(crate) fn scripted(steps: impl IntoIterator<Item = Step>) -> HttpBody {
    HttpBodyBuilder::new_fake().body(Scripted(steps.into_iter().collect()), &HttpBodyOptions::new())
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
