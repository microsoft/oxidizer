// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! When the server middleware reports a request whose response has a body: when the body ends,
//! fails or is dropped, or at handler return for a response without content.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;
use std::time::Duration;

use common::{
    ABANDONED, ACTIVE, COMPLETE, Capture, Captured, DURATION, ERROR, Echoing, Step, data, duration, poll_frame, scripted, server,
};
use future_assert::FutureTestExt;
use http::header::{CONTENT_LENGTH, HeaderValue};
use http::{Method, StatusCode, Version};
use http_body::Body;
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest};
use layered::{Layer, Service};
use observed::{Severity, Value};
use tick::ClockControl;

/// Sends `request` through a server layer to [`Echoing`] with `status`. Returns the response body
/// and a capture of every event.
fn respond(status: StatusCode, request: HttpRequest) -> (HttpBody, Capture) {
    let capture = Capture::new(|_| true);
    let service = server(common::sink_with(&capture)).layer(Echoing(status));
    let body = service.execute(request).unwrap_ready().unwrap().into_body();
    (body, capture)
}

/// A `GET` request over HTTP/1.1 with `body`.
fn get(body: HttpBody) -> HttpRequest {
    common::get().map(|_| body)
}

/// The names of the +1, the event and the −1 of one request.
fn paired(name: &'static str) -> Vec<&'static str> {
    vec![ACTIVE, name, ACTIVE]
}

#[test]
fn size_hint_survives_wrapping() {
    let content_length = |body| respond(StatusCode::OK, get(body)).0.content_length();

    assert_eq!(
        [HttpBodyBuilder::new_fake().text("chunk"), scripted([data("chunk"), Step::End])].map(content_length),
        [Some(5), None]
    );
}

#[test]
fn request_completes_when_body_ends() {
    let capture = Capture::new(|_| true);
    let clock = ClockControl::new();
    let service = server(common::sink_with(&capture))
        .clock(clock.to_simple_clock())
        .layer(Echoing(StatusCode::OK));

    let mut body = service
        .execute(get(scripted([data("chunk"), Step::Pending, Step::End])))
        .unwrap_ready()
        .unwrap()
        .into_body();
    let at_handler_return = capture.names();
    clock.advance(Duration::from_millis(30));
    let _ = poll_frame(&mut body);
    let _ = poll_frame(&mut body);
    let before_end = capture.names();
    clock.advance(Duration::from_millis(20));
    let last = poll_frame(&mut body);

    let complete = capture.events().into_iter().find(|event| event.name == COMPLETE);
    assert_eq!(
        (
            at_handler_return,
            before_end,
            matches!(last, Poll::Ready(None)),
            capture.names(),
            complete.and_then(|event| event.log(DURATION).cloned())
        ),
        (vec![ACTIVE], vec![ACTIVE], true, paired(COMPLETE), Some(Value::U64(50)))
    );
}

#[test]
fn trailers_frame_ends_body() {
    let (mut body, capture) = respond(StatusCode::OK, get(scripted([data("chunk"), Step::Trailers, Step::Pending])));

    let _ = poll_frame(&mut body);
    let before_trailers = capture.names();
    let trailers = matches!(poll_frame(&mut body), Poll::Ready(Some(Ok(frame))) if frame.is_trailers());

    assert_eq!((before_trailers, trailers, capture.names()), (vec![ACTIVE], true, paired(COMPLETE)));
}

#[test]
fn data_frame_that_ends_body_completes_request() {
    let first_poll = |body| {
        let (mut body, capture) = respond(StatusCode::OK, get(body));
        let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
            panic!("the first poll must return a frame");
        };
        (frame.into_data().unwrap().to_vec(), body.is_end_stream(), capture.names())
    };

    assert_eq!(
        [scripted([data("chunk")]), HttpBodyBuilder::new_fake().text("chunk")].map(first_poll),
        [
            (b"chunk".to_vec(), true, paired(COMPLETE)),
            (b"chunk".to_vec(), true, paired(COMPLETE))
        ]
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
            names(Version::HTTP_11, Some("+5"), unfinished()),
            names(Version::HTTP_2, Some("5"), unfinished()),
        ],
        [
            paired(COMPLETE),
            paired(COMPLETE),
            paired(ABANDONED),
            paired(ABANDONED),
            paired(COMPLETE)
        ]
    );
}

#[test]
fn body_dropped_before_end_reports_abandoned_with_status() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(Echoing(StatusCode::PARTIAL_CONTENT));

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
            timestamp: common::sink_time(),
            logs: vec![
                ("http.request.method", Value::from("GET")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(206)),
                ("error.type", Value::from("abandoned")),
                (DURATION, Value::U64(0)),
            ],
            dimensions: vec![
                ("http.request.method", Value::from("GET")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(206)),
                ("error.type", Value::from("abandoned")),
            ],
            measurements: vec![duration(0)],
            enrichments: vec![],
        }
    );
}

#[test]
fn responses_without_content_complete_at_handler_return() {
    let completes_at_handler_return = |method: &Method, version, status, text| {
        let request = common::request(method.clone(), version).map(|_| HttpBodyBuilder::new_fake().text(text));
        let (body, capture) = respond(status, request);
        let names = capture.names();
        drop(body);
        names.contains(&COMPLETE)
    };
    let rows = [
        (Method::HEAD, Version::HTTP_11, StatusCode::OK, "chunk", true),
        (Method::HEAD, Version::HTTP_2, StatusCode::OK, "chunk", true),
        (Method::GET, Version::HTTP_11, StatusCode::EARLY_HINTS, "chunk", true),
        (Method::GET, Version::HTTP_2, StatusCode::EARLY_HINTS, "chunk", true),
        (Method::GET, Version::HTTP_11, StatusCode::NO_CONTENT, "chunk", true),
        (Method::GET, Version::HTTP_2, StatusCode::NO_CONTENT, "chunk", true),
        (Method::GET, Version::HTTP_11, StatusCode::NOT_MODIFIED, "chunk", true),
        (Method::GET, Version::HTTP_2, StatusCode::NOT_MODIFIED, "chunk", true),
        (Method::CONNECT, Version::HTTP_11, StatusCode::OK, "chunk", true),
        (Method::CONNECT, Version::HTTP_2, StatusCode::OK, "chunk", true),
        (Method::GET, Version::HTTP_11, StatusCode::OK, "", true),
        (Method::GET, Version::HTTP_2, StatusCode::OK, "", true),
        (
            Method::CONNECT,
            Version::HTTP_11,
            StatusCode::PROXY_AUTHENTICATION_REQUIRED,
            "chunk",
            false,
        ),
        (Method::GET, Version::HTTP_11, StatusCode::OK, "chunk", false),
        (Method::GET, Version::HTTP_2, StatusCode::OK, "chunk", false),
    ];

    let actual: Vec<_> = rows
        .iter()
        .map(|(method, version, status, text, _)| {
            (
                method.clone(),
                *version,
                *status,
                *text,
                completes_at_handler_return(method, *version, *status, text),
            )
        })
        .collect();
    assert_eq!(actual, rows.to_vec());
}

#[test]
fn body_error_reports_error_event_with_status() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(Echoing(StatusCode::OK));

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
                timestamp: common::sink_time(),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("http.response.status_code", Value::I64(200)),
                    ("error.type", Value::from("validation")),
                    (DURATION, Value::U64(0)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("http.response.status_code", Value::I64(200)),
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
    let service = server(sink).layer(Echoing(StatusCode::OK));

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
        (Some("the body panicked"), ERROR, Some(Value::I64(200)), Some(Value::from("panic")))
    );
}

#[test]
fn body_failure_label_wins_over_server_error_status() {
    // Each body gets one poll and is then dropped.
    let error_type = |steps: Vec<Step>| {
        let (sink, capture) = common::capture_sink();
        let service = server(sink).layer(Echoing(StatusCode::SERVICE_UNAVAILABLE));
        let mut body = service.execute(get(scripted(steps))).unwrap_ready().unwrap().into_body();
        let _ = catch_unwind(AssertUnwindSafe(|| poll_frame(&mut body)));
        drop(body);
        capture.single().log("error.type").cloned()
    };

    assert_eq!(
        [
            error_type(vec![Step::Error(HttpError::validation("the page is missing"))]),
            error_type(vec![Step::Panic]),
            error_type(vec![data("chunk"), Step::Pending]),
            error_type(vec![data("chunk")]),
        ],
        ["validation", "panic", "abandoned", "503"].map(|label| Some(Value::from(label)))
    );
}
