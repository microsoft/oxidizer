// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! When the server middleware reports a request whose response has a body: when the body ends,
//! fails or is dropped, or at handler return for a response without content. The `error.type` of
//! a body that fails or is dropped is the label of the failure, also for a status from 500 to 599.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;
use std::time::Duration;

use common::{ABANDONED, COMPLETE, Capture, Captured, DURATION, ERROR, Echoing, Step, data, duration, poll_frame, scripted, server};
use future_assert::FutureTestExt;
use http::header::{CONTENT_LENGTH, HeaderValue};
use http::{Method, StatusCode, Version};
use http_body::Body;
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest};
use layered::{Layer, Service};
use observed::{Severity, Value};
use tick::ClockControl;

/// Sends `request` through a server layer to [`Echoing`] with `status`. Returns the response body
/// and a capture of every event except the deltas of `http.server.active_requests`.
fn respond(status: StatusCode, request: HttpRequest) -> (HttpBody, Capture) {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(Echoing(status));
    let body = service.execute(request).unwrap_ready().unwrap().into_body();
    (body, capture)
}

/// A `GET` request over HTTP/1.1 with `body`.
fn get(body: HttpBody) -> HttpRequest {
    common::get().map(|_| body)
}

#[test]
fn size_hint_survives_wrapping() {
    let (body, _capture) = respond(StatusCode::OK, get(HttpBodyBuilder::new_fake().text("chunk")));

    assert_eq!(body.content_length(), Some(5));
}

#[test]
fn request_completes_when_body_ends() {
    let complete = |method: Method, status: StatusCode| {
        let (sink, capture) = common::capture_sink();
        let clock = ClockControl::new();
        let service = server(sink).clock(clock.to_simple_clock()).layer(Echoing(status));

        let request = common::request(method, Version::HTTP_11).map(|_| scripted([data("chunk"), Step::Pending, Step::End]));
        let mut body = service.execute(request).unwrap_ready().unwrap().into_body();
        clock.advance(Duration::from_millis(30));
        let _ = poll_frame(&mut body);
        let _ = poll_frame(&mut body);
        clock.advance(Duration::from_millis(20));
        let last = poll_frame(&mut body);

        let event = capture.single();
        (matches!(last, Poll::Ready(None)), event.name, event.log(DURATION).cloned())
    };

    assert_eq!(
        [
            complete(Method::GET, StatusCode::OK),
            complete(Method::CONNECT, StatusCode::PROXY_AUTHENTICATION_REQUIRED)
        ]
        .to_vec(),
        vec![(true, COMPLETE, Some(Value::U64(50))); 2]
    );
}

#[test]
fn trailers_frame_ends_body() {
    let (mut body, capture) = respond(StatusCode::OK, get(scripted([data("chunk"), Step::Trailers, Step::Pending])));

    let _ = poll_frame(&mut body);
    let trailers = matches!(poll_frame(&mut body), Poll::Ready(Some(Ok(frame))) if frame.is_trailers());

    assert_eq!((trailers, capture.names()), (true, vec![COMPLETE]));
}

#[test]
fn data_frame_that_ends_body_completes_request() {
    let (mut body, capture) = respond(StatusCode::OK, get(scripted([data("chunk")])));

    let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
        panic!("the first poll must return a frame");
    };

    assert_eq!(
        (frame.into_data().unwrap().to_vec(), body.is_end_stream(), capture.names()),
        (b"chunk".to_vec(), true, vec![COMPLETE])
    );
}

#[test]
fn drop_completes_ended_body_and_abandons_unfinished_body() {
    // Each body gets two polls and is then dropped.
    let names = |version, content_length: Option<&'static str>, steps: Vec<Step>| {
        let mut request = common::request(Method::GET, version).map(|_| scripted(steps));
        if let Some(content_length) = content_length {
            request
                .headers_mut()
                .insert(CONTENT_LENGTH, HeaderValue::from_static(content_length));
        }
        let (mut body, capture) = respond(StatusCode::OK, request);
        let _ = poll_frame(&mut body);
        let _ = poll_frame(&mut body);
        drop(body);
        capture.names()
    };
    let unfinished = || vec![data("hello"), Step::Pending, Step::Pending];

    assert_eq!(
        [
            names(Version::HTTP_11, None, vec![data("hello"), Step::Pending]),
            names(Version::HTTP_11, Some("5"), unfinished()),
            names(Version::HTTP_11, Some("10"), unfinished()),
            names(Version::HTTP_2, Some("5"), vec![data("hello"), Step::Pending, Step::Trailers]),
        ],
        [vec![COMPLETE], vec![COMPLETE], vec![ABANDONED], vec![ABANDONED]]
    );
}

#[test]
fn body_dropped_before_end_reports_abandoned_with_status() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(Echoing(StatusCode::SERVICE_UNAVAILABLE));

    let mut body = service
        .execute(get(scripted([data("chunk"), Step::Pending])))
        .unwrap_ready()
        .unwrap()
        .into_body();
    let _ = poll_frame(&mut body);
    drop(body);

    assert_eq!(
        capture.single(),
        Captured {
            name: ABANDONED,
            severity: Some(Severity::Warn),
            message: Some("HTTP server request abandoned".into()),
            logs: vec![
                ("http.request.method", Value::from("GET")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(503)),
                ("error.type", Value::from("abandoned")),
                (DURATION, Value::U64(0)),
            ],
            dimensions: vec![
                ("http.request.method", Value::from("GET")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(503)),
                ("error.type", Value::from("abandoned")),
            ],
            measurements: vec![duration(0)],
            enrichments: vec![],
        }
    );
}

#[test]
fn responses_without_content_complete_at_handler_return() {
    let names_at_handler_return = |(method, status, text): (Method, StatusCode, &'static str)| {
        let request = common::request(method, Version::HTTP_11).map(|_| HttpBodyBuilder::new_fake().text(text));
        let (body, capture) = respond(status, request);
        let names = capture.names();
        drop(body);
        names
    };
    let rows = [
        (Method::HEAD, StatusCode::OK, "chunk"),
        (Method::GET, StatusCode::EARLY_HINTS, "chunk"),
        (Method::GET, StatusCode::NO_CONTENT, "chunk"),
        (Method::GET, StatusCode::NOT_MODIFIED, "chunk"),
        (Method::CONNECT, StatusCode::OK, "chunk"),
        (Method::GET, StatusCode::OK, ""),
    ];

    assert_eq!(rows.map(names_at_handler_return).to_vec(), vec![vec![COMPLETE]; 6]);
}

#[test]
fn body_error_reports_error_event_with_status() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(Echoing(StatusCode::SERVICE_UNAVAILABLE));

    let mut body = service
        .execute(get(scripted([
            data("chunk"),
            Step::Error(HttpError::validation("the page is missing")),
        ])))
        .unwrap_ready()
        .unwrap()
        .into_body();
    let _ = poll_frame(&mut body);
    let Poll::Ready(Some(Err(error))) = poll_frame(&mut body) else {
        panic!("the second poll must return the error of the body");
    };

    assert_eq!(
        (error.to_string(), capture.single()),
        (
            "the page is missing".to_owned(),
            Captured {
                name: ERROR,
                severity: Some(Severity::Error),
                message: Some("HTTP server request failed".into()),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("http.response.status_code", Value::I64(503)),
                    ("error.type", Value::from("validation")),
                    (DURATION, Value::U64(0)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("http.response.status_code", Value::I64(503)),
                    ("error.type", Value::from("validation")),
                ],
                measurements: vec![duration(0)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn body_panic_reports_panic_error_with_status() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(Echoing(StatusCode::SERVICE_UNAVAILABLE));

    let mut body = service.execute(get(scripted([Step::Panic]))).unwrap_ready().unwrap().into_body();
    let payload = catch_unwind(AssertUnwindSafe(|| poll_frame(&mut body))).unwrap_err();

    let event = capture.single();
    assert_eq!(
        (
            payload.downcast_ref::<&str>().copied(),
            event.name,
            event.log("http.response.status_code").cloned(),
            event.log("error.type").cloned()
        ),
        (Some("the body panicked"), ERROR, Some(Value::I64(503)), Some(Value::from("panic")))
    );
}
