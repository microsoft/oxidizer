// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The header fields of the server events.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::sync::Arc;

use common::{COMPLETE, Capture, Captured, DURATION, Echoing, PUBLIC, duration, server};
use data_privacy::RedactionEngine;
use future_assert::FutureTestExt;
use http::header::RETRY_AFTER;
use http::{HeaderName, HeaderValue, StatusCode};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::TelemetryLayer;
use layered::{Execute, Layer, Service};
use observed::{Severity, Sink, SinkId, Value};
use tick::SimpleClock;

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const TAG: HeaderName = HeaderName::from_static("x-tag");

fn get(headers: &[(HeaderName, &'static [u8])]) -> HttpRequest {
    let mut request = common::get_to("http://example.com/books/1");
    for (name, value) in headers {
        request.headers_mut().append(name, HeaderValue::from_bytes(value).unwrap());
    }
    request
}

#[test]
fn configured_headers_are_log_fields_with_joined_values() {
    let (sink, capture) = common::capture_sink();
    let utf8 = HeaderName::from_static("x-utf8");
    server(sink)
        .request_header_data_classes([
            (REQUEST_ID, PUBLIC),
            (TAG, PUBLIC),
            (HeaderName::from_static("x-absent"), PUBLIC),
            (utf8.clone(), PUBLIC),
        ])
        .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
        .layer(Echoing(StatusCode::OK))
        .execute(get(&[
            (REQUEST_ID, b"7"),
            (TAG, b"a"),
            (TAG, b"caf\xe9"),
            (TAG, b"b"),
            (utf8, "café".as_bytes()),
            (RETRY_AFTER, b"120"),
        ]))
        .unwrap_ready()
        .unwrap();

    let dimensions = vec![
        ("http.request.method", Value::from("GET")),
        ("url.scheme", Value::from("http")),
        ("network.protocol.version", Value::from("1.1")),
        ("server.address", Value::from("example.com")),
        ("server.port", Value::I64(80)),
        ("http.response.status_code", Value::I64(200)),
    ];
    let mut logs = dimensions.clone();
    logs.extend([
        (DURATION, Value::F64(0.0)),
        ("http.request.header.x-request-id", Value::from("7")),
        ("http.request.header.x-tag", Value::from("a, caf\u{fffd}, b")),
        ("http.request.header.x-utf8", Value::from("café")),
        ("http.response.header.retry-after", Value::from("120")),
    ]);
    assert_eq!(
        capture.single(),
        Captured {
            name: COMPLETE,
            severity: Some(Severity::Info),
            message: Some("HTTP server request completed".into()),
            logs,
            dimensions,
            measurements: vec![duration(0.0)],
            enrichments: vec![],
        }
    );
}

#[test]
fn header_values_are_taken_when_request_starts_and_when_handler_returns() {
    let capture = Capture::new(|name| name == COMPLETE);
    let bodies = HttpBodyBuilder::new_fake();
    let handler = Execute::new(move |mut request: HttpRequest| {
        request.headers_mut().insert(REQUEST_ID, HeaderValue::from_static("handler"));
        let mut response = HttpResponse::new(bodies.empty());
        response.headers_mut().insert(REQUEST_ID, HeaderValue::from_static("response"));
        async move { Ok::<_, HttpError>(response) }
    });

    server(common::sink_with(&capture))
        .request_header_data_classes([(REQUEST_ID, PUBLIC)])
        .response_header_data_classes([(REQUEST_ID, PUBLIC)])
        .layer(handler)
        .execute(get(&[(REQUEST_ID, b"request")]))
        .unwrap_ready()
        .unwrap();

    let event = capture.single();
    assert_eq!(
        ["http.request.header.x-request-id", "http.response.header.x-request-id"].map(|key| event.log(key).cloned()),
        [Some(Value::from("request")), Some(Value::from("response"))]
    );
}

#[test]
fn request_or_response_headers_alone_are_logged() {
    let logged = |configure: fn(TelemetryLayer) -> TelemetryLayer| {
        let capture = Capture::new(|name| name == COMPLETE);
        configure(server(common::sink_with(&capture)))
            .layer(Echoing(StatusCode::OK))
            .execute(get(&[(REQUEST_ID, b"7"), (RETRY_AFTER, b"120")]))
            .unwrap_ready()
            .unwrap();
        let event = capture.single();
        ["http.request.header.x-request-id", "http.response.header.retry-after"].map(|key| event.log(key).cloned())
    };

    assert_eq!(
        [
            logged(|layer| layer.request_header_data_classes([(REQUEST_ID, PUBLIC)])),
            logged(|layer| layer.response_header_data_classes([(RETRY_AFTER, PUBLIC)])),
        ],
        [[Some(Value::from("7")), None], [None, Some(Value::from("120"))]]
    );
}

#[test]
fn each_processor_redacts_headers_with_its_own_redactor() {
    let printing = Capture::new(|name| name == COMPLETE);
    let erasing = Capture::with_engine(|name| name == COMPLETE, RedactionEngine::default());
    let sink = Sink::builder(SinkId::new("two"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(printing.clone()), Arc::new(erasing.clone())])
        .build();

    server(sink)
        .request_header_data_classes([(REQUEST_ID, PUBLIC)])
        .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
        .layer(Echoing(StatusCode::OK))
        .execute(get(&[(REQUEST_ID, b"7"), (RETRY_AFTER, b"120")]))
        .unwrap_ready()
        .unwrap();

    let headers = |capture: Capture| {
        let event = capture.single();
        ["http.request.header.x-request-id", "http.response.header.retry-after"].map(|key| event.log(key).cloned())
    };
    assert_eq!(
        [printing, erasing].map(headers),
        [
            [Some(Value::from("7")), Some(Value::from("120"))],
            [Some(Value::from("")), Some(Value::from(""))]
        ]
    );
}
