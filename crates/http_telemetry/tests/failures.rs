// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server and client middleware when the handler returns an error or panics,
//! when a processor panics, and for error statuses.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::future::{Ready, poll_fn, ready};
use std::io::{self, ErrorKind};
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::Arc;
use std::task::Poll;

use common::{
    ACTIVE, CLIENT_DURATION, CLIENT_ERROR, COMPLETE, Capture, Captured, DURATION, ERROR, TOTAL_ERROR, client, client_duration, duration,
    server,
};
use future_assert::FutureTestExt;
use http::StatusCode;
use http_extensions::{HttpError, HttpRequest, HttpResponse};
use layered::{Execute, Layer, Service};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{FlushError, Severity, Sink, SinkId, Value};
use ohno::ErrorLabel;
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
                    (DURATION, Value::F64(0.0)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("error.type", Value::from("validation")),
                ],
                measurements: vec![duration(0.0)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn chained_handler_error_reports_labels_of_the_chain() {
    let error_type_of = |error: fn() -> HttpError| {
        let (sink, capture) = common::capture_sink();
        let handler = Execute::new(move |_: HttpRequest| ready(Err::<HttpResponse, _>(error())));
        server(sink).layer(handler).execute(common::get()).unwrap_ready().unwrap_err();
        error_type(&capture.single())
    };

    assert_eq!(
        [
            error_type_of(|| HttpError::from(io::Error::from(ErrorKind::ConnectionRefused))),
            error_type_of(|| HttpError::from(io::Error::other(io::Error::from(ErrorKind::ConnectionReset)))),
            error_type_of(|| HttpError::from(io::Error::new(ErrorKind::PermissionDenied, "access denied"))),
        ],
        ["io.connection_refused", "io.connection_reset", "io.permission_denied"]
            .map(|label| (Some(label.to_owned()), Some(label.to_owned())))
    );
}

#[test]
fn handler_panic_reports_panic_error_and_resumes_unwinding() {
    let (sink, capture) = common::capture_sink();
    let handler =
        || Execute::new(|_: HttpRequest| poll_fn(|_| -> Poll<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") }));
    let services = [
        server(sink.clone()).layer(handler()),
        client(sink.clone()).layer(handler()),
        client(sink).report_total_duration(true).layer(handler()),
    ];

    let payloads = services.map(|service| catch_unwind(AssertUnwindSafe(|| service.execute(common::get()).unwrap_ready())).unwrap_err());

    let events: Vec<_> = capture.events().iter().map(|event| (event.name, error_type(event))).collect();
    assert_eq!(
        (payloads.each_ref().map(|payload| payload.downcast_ref::<&str>()), events),
        (
            [Some(&"the handler panicked"); 3],
            [ERROR, CLIENT_ERROR, TOTAL_ERROR]
                .map(|name| (name, (Some("panic".to_owned()), Some("panic".to_owned()))))
                .to_vec()
        )
    );
}

#[test]
fn synchronous_handler_panic_reports_panic_error() {
    let (sink, capture) = common::capture_sink();
    let handler = || Execute::new(|_: HttpRequest| -> Ready<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") });
    let services = [server(sink.clone()).layer(handler()), client(sink).layer(handler())];

    let payloads = services.map(|service| catch_unwind(AssertUnwindSafe(|| drop(service.execute(common::get())))).unwrap_err());

    let events: Vec<_> = capture.events().iter().map(|event| (event.name, error_type(event))).collect();
    assert_eq!(
        (payloads.each_ref().map(|payload| payload.downcast_ref::<&str>()), events),
        (
            [Some(&"the handler panicked"); 2],
            [ERROR, CLIENT_ERROR]
                .map(|name| (name, (Some("panic".to_owned()), Some("panic".to_owned()))))
                .to_vec()
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

#[test]
fn client_handler_error_reports_error_event_with_label() {
    let capture = Capture::new(|name| name == CLIENT_ERROR);
    let service = client(common::sink_with(&capture)).layer(common::FAILING);

    let error = service.execute(common::get_book()).unwrap_ready().unwrap_err();

    assert_eq!(
        (error.to_string(), capture.single()),
        (
            "the review has no text".to_owned(),
            Captured {
                name: CLIENT_ERROR,
                severity: Some(Severity::Warn),
                message: Some("HTTP request attempt failed".into()),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("url.template", Value::from("/books/{book}")),
                    ("url.path.redacted", Value::from("/books/1")),
                    ("error.type", Value::from("validation")),
                    ("exception.message", Value::from("the review has no text")),
                    (CLIENT_DURATION, Value::F64(0.0)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("url.template", Value::from("/books/{book}")),
                    ("error.type", Value::from("validation")),
                ],
                measurements: vec![client_duration(0.0)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn error_labels_with_replaces_the_labels_of_handler_errors() {
    let (sink, capture) = common::capture_sink();
    let service = client(sink)
        .error_labels_with(|_| ErrorLabel::from_static("tls"))
        .layer(common::FAILING_CHAINED);

    service.execute(common::get()).unwrap_ready().unwrap_err();

    assert_eq!(error_type(&capture.single()), (Some("tls".to_owned()), Some("tls".to_owned())));
}
