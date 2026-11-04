// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server middleware when the handler returns an error or panics, when a
//! processor panics, and for error statuses.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::future::{Ready, poll_fn, ready};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::task::Poll;

use common::{ACTIVE, COMPLETE, Capture, Captured, DURATION, ERROR, duration, server};
use future_assert::{FutureTestExt, poll_once};
use http::{StatusCode, Uri};
use http_extensions::{HttpError, HttpRequest, HttpResponse, collect_error_labels};
use layered::{Execute, Layer, Service};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{FlushError, Severity, Sink, SinkId, Value};
use tick::SimpleClock;

/// Panics for each event that it gets, except the deltas of `http.server.active_requests`.
struct PanickingProcessor;

impl EventProcessor for PanickingProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() != ACTIVE
    }

    #[expect(clippy::panic, reason = "this test injects a processor panic")]
    fn process(&self, _: &EventView<'_>) {
        panic!("the processor panicked");
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

/// The `error.type` of `event`, as a log field and as a metric dimension.
fn error_type(event: &Captured) -> (Option<String>, Option<String>) {
    (
        event.log("error.type").map(ToString::to_string),
        event.dimension("error.type").map(ToString::to_string),
    )
}

#[test]
fn server_handler_error_reports_error_event_without_status() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::FAILING);

    let error = service
        .execute(common::get_to("https://example.com/books/1"))
        .unwrap_ready()
        .unwrap_err();

    assert_eq!(
        (error.to_string(), capture.single()),
        (
            "the review has no text".to_owned(),
            Captured {
                name: ERROR,
                severity: Some(Severity::Error),
                message: Some("HTTP server request failed".into()),
                timestamp: common::sink_time(),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("error.type", Value::from("validation")),
                    (DURATION, Value::U64(0)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("error.type", Value::from("validation")),
                ],
                measurements: vec![duration(0)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn chained_handler_error_label_matches_collect_error_labels() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::FAILING_CHAINED);

    let error = service.execute(common::get()).unwrap_ready().unwrap_err();

    let label = "io.connection_refused".to_owned();
    assert_eq!(
        (error_type(&capture.single()), collect_error_labels(&error).to_string()),
        ((Some(label.clone()), Some(label.clone())), label)
    );
}

#[test]
fn original_error_is_returned_unchanged() {
    let (sink, _capture) = common::capture_sink();
    let handler = Execute::new(|request: HttpRequest| {
        ready(Err::<HttpResponse, _>(
            HttpError::validation("the review has no text").with_request(request),
        ))
    });
    let service = server(sink).layer(handler);

    let mut error = service.execute(common::get()).unwrap_ready().unwrap_err();

    assert_eq!(
        error.take_request().map(|request| request.uri().clone()),
        Some(Uri::from_static("/books/1"))
    );
}

#[test]
fn server_handler_panic_reports_panic_error_and_resumes_unwinding() {
    let (sink, capture) = common::capture_sink();
    let handler =
        Execute::new(|_: HttpRequest| poll_fn(|_| -> Poll<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") }));
    let service = server(sink).layer(handler);

    let payload = catch_unwind(AssertUnwindSafe(|| service.execute(common::get()).unwrap_ready())).unwrap_err();

    let event = capture.single();
    assert_eq!(
        (payload.downcast_ref::<&str>(), event.name, error_type(&event)),
        (
            Some(&"the handler panicked"),
            ERROR,
            (Some("panic".to_owned()), Some("panic".to_owned()))
        )
    );
}

#[test]
fn synchronous_handler_panic_reports_panic_error() {
    let (sink, capture) = common::capture_sink();
    let handler = Execute::new(|_: HttpRequest| -> Ready<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") });
    let service = server(sink).layer(handler);

    let payload = catch_unwind(AssertUnwindSafe(|| drop(service.execute(common::get())))).unwrap_err();

    let event = capture.single();
    assert_eq!(
        (payload.downcast_ref::<&str>(), event.name, error_type(&event)),
        (
            Some(&"the handler panicked"),
            ERROR,
            (Some("panic".to_owned()), Some("panic".to_owned()))
        )
    );
}

#[test]
fn processor_panic_is_not_reported_as_handler_panic() {
    let capture = Capture::new(|name| name != ACTIVE);
    let sink = Sink::builder(SinkId::new("panicking"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(capture.clone()), Arc::new(PanickingProcessor)])
        .build();
    let service = server(sink).layer(common::responding(StatusCode::OK));

    let payload = catch_unwind(AssertUnwindSafe(|| service.execute(common::get()).unwrap_ready())).unwrap_err();

    assert_eq!(
        (payload.downcast_ref::<&str>(), capture.names()),
        (Some(&"the processor panicked"), vec![COMPLETE])
    );
}

#[test]
fn server_error_status_sets_error_type_to_status_code() {
    let (sink, capture) = common::capture_sink();
    let statuses: Vec<_> = (500..=599).map(|code| StatusCode::from_u16(code).unwrap()).collect();

    for status in &statuses {
        let service = server(sink.clone()).layer(common::responding(*status));
        service.execute(common::get()).unwrap_ready().unwrap();
    }

    let error_types: Vec<_> = capture.events().iter().map(error_type).collect();
    let expected: Vec<_> = statuses
        .iter()
        .map(|status| (Some(status.as_str().to_owned()), Some(status.as_str().to_owned())))
        .collect();
    assert_eq!(error_types, expected);
}

#[test]
fn status_below_500_has_no_error_type() {
    let (sink, capture) = common::capture_sink();

    for status in [StatusCode::OK, StatusCode::NOT_FOUND, StatusCode::from_u16(499).unwrap()] {
        let service = server(sink.clone()).layer(common::responding(status));
        service.execute(common::get()).unwrap_ready().unwrap();
    }

    let error_types: Vec<_> = capture.events().iter().map(error_type).collect();
    assert_eq!(error_types, vec![(None, None); 3]);
}

#[test]
fn request_dropped_during_unwinding_emits_nothing() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::NeverResponding);
    let mut future = Box::pin(service.execute(common::get()));
    assert!(poll_once(&mut future).is_pending());

    let unwinding = catch_unwind(AssertUnwindSafe(move || {
        let _future = future;
        panic!("the caller panicked");
    }));

    assert_eq!((unwinding.is_err(), capture.names()), (true, Vec::<&str>::new()));
}
