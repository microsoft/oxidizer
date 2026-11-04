// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server middleware when the handler returns an error or panics, when a
//! processor panics, and for error statuses.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::future::{Ready, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::Arc;
use std::task::Poll;

use common::{ACTIVE, COMPLETE, Capture, Captured, DURATION, ERROR, duration, server};
use future_assert::FutureTestExt;
use http::StatusCode;
use http_extensions::{HttpRequest, HttpResponse};
use layered::{Execute, Layer, Service};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{FlushError, Severity, Sink, SinkId, Value};
use tick::SimpleClock;

/// Panics with the name of each event that it gets, except the deltas of
/// `http.server.active_requests`.
struct PanickingProcessor;

impl EventProcessor for PanickingProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() != ACTIVE
    }

    #[expect(clippy::panic, reason = "this test injects a processor panic")]
    fn process(&self, event: &EventView<'_>) {
        panic_any(event.name());
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
    let capture = Capture::new(|name| name == ERROR);
    let service = server(common::sink_with(&capture)).layer(common::FAILING);

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
fn chained_handler_error_reports_labels_of_the_chain() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::FAILING_CHAINED);

    service.execute(common::get()).unwrap_ready().unwrap_err();

    let label = Some("io.connection_refused".to_owned());
    assert_eq!(error_type(&capture.single()), (label.clone(), label));
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
fn processor_panic_reaches_caller() {
    let sink = Sink::builder(SinkId::new("panicking"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(PanickingProcessor)])
        .build();
    let service = server(sink).layer(common::responding(StatusCode::OK));

    let payload = catch_unwind(AssertUnwindSafe(|| service.execute(common::get()).unwrap_ready())).unwrap_err();

    assert_eq!(payload.downcast_ref::<&str>(), Some(&COMPLETE));
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
