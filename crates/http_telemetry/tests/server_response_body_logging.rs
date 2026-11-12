// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The response body that the server middleware logs as `http.response.body`.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::task::Poll;

use bytesbuf::BytesView;
use common::{ABANDONED, COMPLETE, Capture, Captured, DURATION, ERROR, Echoing, Step, data, duration, poll_frame, scripted, server};
use future_assert::FutureTestExt;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, StatusCode, Version};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest};
use http_telemetry::{Telemetry, TelemetryLayer};
use layered::{Layer, Service};
use observed::{Severity, Sink, Value};
use ohno::ErrorExt;

const BODY: &str = "http.response.body";

/// Sends `request` through a server layer that `configure` sets up, to [`Echoing`] with 200. Returns the response
/// body and a capture of every event except the deltas of `http.server.active_requests`.
fn respond(configure: impl FnOnce(TelemetryLayer) -> TelemetryLayer, request: HttpRequest) -> (HttpBody, Capture) {
    let (sink, capture) = common::capture_sink();
    let service = configure(server(sink)).layer(Echoing(StatusCode::OK));
    let body = service.execute(request).unwrap_ready().unwrap().into_body();
    (body, capture)
}

/// A server layer that logs the bodies of `types`.
fn logging(types: &[&'static str]) -> impl FnOnce(TelemetryLayer) -> TelemetryLayer {
    move |layer| layer.response_body_media_types(types.iter().copied()).unwrap()
}

/// A request with `method` over HTTP/1.1, with `body`, and with `content_type` as its `Content-Type`. [`Echoing`]
/// responds with both.
fn request(method: Method, content_type: &'static str, body: HttpBody) -> HttpRequest {
    let mut request = common::request(method, Version::HTTP_11).map(|_| body);
    request.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    request
}

/// Polls `body` until it ends, and describes each frame: the text of a data frame, or `trailers`.
fn read_to_end(mut body: HttpBody) -> Vec<String> {
    let mut frames = Vec::new();
    while let Poll::Ready(Some(frame)) = poll_frame(&mut body) {
        frames.push(match frame.unwrap().into_data() {
            Ok(data) => String::from_utf8_lossy(&data.to_vec()).into_owned(),
            Err(_) => "trailers".to_owned(),
        });
    }
    frames
}

#[test]
fn matching_body_is_log_only_field() {
    let (body, capture) = respond(
        logging(&["application/json"]),
        request(Method::GET, "application/json", HttpBodyBuilder::new_fake().text(r#"{"book":1}"#)),
    );
    read_to_end(body);

    let dimensions = vec![
        ("http.request.method", Value::from("GET")),
        ("network.protocol.version", Value::from("1.1")),
        ("http.response.status_code", Value::I64(200)),
    ];
    let mut logs = dimensions.clone();
    logs.extend([(DURATION, Value::F64(0.0)), (BODY, Value::from(r#"{"book":1}"#))]);
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
fn body_is_logged_only_when_content_type_matches() {
    let logged = |types: &[&'static str], content_type: Option<&'static str>| {
        let mut request = common::get().map(|_| HttpBodyBuilder::new_fake().text("chunk"));
        if let Some(content_type) = content_type {
            request.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        }
        let (body, capture) = respond(logging(types), request);
        read_to_end(body);
        capture.single().log(BODY).is_some()
    };
    let rows: [(&[&'static str], Option<&'static str>, bool); 18] = [
        (&["application/json"], Some("application/json"), true),
        (&["application/json"], Some("text/plain"), false),
        (&["application/json"], None, false),
        (&[], Some("application/json"), false),
        (&["text/*"], Some("text/csv"), true),
        (&["text/*"], Some("application/json"), false),
        (&["*/*"], Some("image/png"), true),
        (&["application/json"], Some("application/ld+json"), true),
        (&["application/xml"], Some("application/soap+xml"), true),
        (&["application/soap"], Some("application/soap+xml"), false),
        (&["application/ld+json"], Some("application/ld+json"), true),
        (&["application/ld+json"], Some("application/ld+xml"), false),
        (&["application/ld+json"], Some("application/foo+ld"), false),
        (&["application/*+json"], Some("application/pdf"), false),
        (&["application/json"], Some("Application/JSON ; charset=utf-8"), true),
        (&["APPLICATION/JSON"], Some("application/json"), true),
        (&["text/plain; charset=utf-8"], Some("text/plain"), true),
        (&["application/json"], Some("not a media type"), false),
    ];

    assert_eq!(
        rows.map(|(types, content_type, _)| (types, content_type, logged(types, content_type))),
        rows
    );
}

#[test]
fn responses_without_content_log_empty_text() {
    let logged = |method: Method, text: &str| {
        let (_body, capture) = respond(
            logging(&["application/json"]),
            request(method, "application/json", HttpBodyBuilder::new_fake().text(text)),
        );
        capture.single().log(BODY).cloned()
    };

    assert_eq!(
        [logged(Method::GET, ""), logged(Method::HEAD, "chunk")],
        [Some(Value::from("")), Some(Value::from(""))]
    );
}

#[test]
fn logged_text_is_cut_at_limit_and_lossy() {
    let logged = |limit: usize, steps: Vec<Step>| {
        let (body, capture) = respond(
            |layer| logging(&["text/plain"])(layer).body_size_limit(limit),
            request(Method::GET, "text/plain", scripted(steps)),
        );
        read_to_end(body);
        capture.single().log(BODY).cloned()
    };
    let invalid = Step::Data(BytesView::copied_from_slice(b"caf\xe9", &HttpBodyBuilder::new_fake()));

    assert_eq!(
        [
            logged(5, vec![data("hel"), data("lo wo"), data("rld")]),
            logged(4, vec![data("café")]),
            logged(32, vec![invalid]),
        ],
        [
            Some(Value::from("hello")),
            Some(Value::from("caf\u{fffd}")),
            Some(Value::from("caf\u{fffd}"))
        ]
    );
}

#[test]
fn default_limit_is_32_kib() {
    let (body, capture) = respond(
        logging(&["text/plain"]),
        request(Method::GET, "text/plain", HttpBodyBuilder::new_fake().text("a".repeat(32_769))),
    );
    read_to_end(body);

    assert_eq!(capture.single().log(BODY).cloned(), Some(Value::from("a".repeat(32_768))));
}

#[test]
fn client_receives_every_frame_of_logged_body() {
    let (body, capture) = respond(
        |layer| logging(&["text/plain"])(layer).body_size_limit(5),
        request(Method::GET, "text/plain", scripted([data("hello"), data(" world"), Step::Trailers])),
    );

    assert_eq!(
        (read_to_end(body), capture.single().log(BODY).cloned()),
        (
            vec!["hello".to_owned(), " world".to_owned(), "trailers".to_owned()],
            Some(Value::from("hello"))
        )
    );
}

#[test]
fn failed_and_abandoned_bodies_log_bytes_read_before_end() {
    let logged = |steps: Vec<Step>| {
        let (mut body, capture) = respond(logging(&["text/plain"]), request(Method::GET, "text/plain", scripted(steps)));
        let _ = poll_frame(&mut body);
        let _ = poll_frame(&mut body);
        drop(body);
        let event = capture.single();
        (event.name, event.log(BODY).cloned())
    };

    assert_eq!(
        [
            logged(vec![data("hello"), Step::Error(HttpError::validation("the page is missing"))]),
            logged(vec![data("hello"), Step::Pending, Step::Pending]),
        ],
        [(ERROR, Some(Value::from("hello"))), (ABANDONED, Some(Value::from("hello")))]
    );
}

#[test]
fn invalid_media_type_returns_error() {
    let error = Telemetry::server(Sink::noop())
        .response_body_media_types(["application/json", "json"])
        .unwrap_err();

    assert_eq!(error.message(), "'json' is not a media type");
}
