// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The request body that the server middleware copies while the handler reads it, and logs as `http.request.body`.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use common::{ABANDONED, COMPLETE, Capture, Captured, DURATION, ERROR, Echoing, Step, data, duration, poll_frame, scripted, server};
use future_assert::{FutureTestExt, poll_once};
use futures::{StreamExt, stream};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, StatusCode, Version};
use http_body::Body;
use http_extensions::{HttpBody, HttpBodyBuilder, HttpBodyOptions, HttpError, HttpRequest, HttpResponse, RequestHandler};
use http_telemetry::{Telemetry, TelemetryLayer};
use layered::{Execute, Layer, Service};
use observed::{Severity, Sink, Value};
use ohno::{ErrorExt, Labeled};
use tick::ClockControl;

const BODY: &str = "http.request.body";

/// A server layer that logs the request bodies of `types`.
fn logging(types: &[&'static str]) -> impl FnOnce(TelemetryLayer) -> TelemetryLayer {
    move |layer| layer.request_body_media_types(types.iter().copied()).unwrap()
}

/// Sends `request` through a server layer that `configure` sets up, to `handler`, which responds at the first poll.
/// Returns the response body and a capture of every event except the deltas of `http.server.active_requests`.
fn respond(
    configure: impl FnOnce(TelemetryLayer) -> TelemetryLayer,
    handler: impl RequestHandler,
    request: HttpRequest,
) -> (HttpBody, Capture) {
    let (sink, capture) = common::capture_sink();
    let service = configure(server(sink)).layer(handler);
    let body = service.execute(request).unwrap_ready().unwrap().into_body();
    (body, capture)
}

/// A `POST` request over HTTP/1.1 with `body`, and with `content_type` as its `Content-Type`.
fn request(content_type: &'static str, body: HttpBody) -> HttpRequest {
    let mut request = common::request(Method::POST, Version::HTTP_11).map(|_| body);
    request.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    request
}

/// Polls `body` until it ends, and describes each frame: the text of a data frame, `trailers`, or the label of an
/// error.
fn read_to_end(mut body: HttpBody) -> Vec<String> {
    let mut frames = Vec::new();
    while let Poll::Ready(Some(frame)) = poll_frame(&mut body) {
        frames.push(match frame {
            Ok(frame) => match frame.into_data() {
                Ok(data) => String::from_utf8_lossy(&data.to_vec()).into_owned(),
                Err(_) => "trailers".to_owned(),
            },
            Err(error) => error.label().to_string(),
        });
    }
    frames
}

/// A handler that keeps the request body that it gets in `kept`, and responds with 200 and an empty body.
fn keeping(kept: &Arc<Mutex<Option<HttpBody>>>) -> impl RequestHandler {
    let kept = Arc::clone(kept);
    Execute::new(move |request: HttpRequest| {
        *kept.lock().unwrap() = Some(request.into_body());
        std::future::ready(Ok::<_, HttpError>(HttpResponse::new(HttpBodyBuilder::new_fake().empty())))
    })
}

/// A handler that reads the request body to its end, and then responds with 200 and an empty body.
fn reading() -> impl RequestHandler {
    Execute::new(|request: HttpRequest| async move {
        request.into_body().into_bytes().await?;
        Ok::<_, HttpError>(HttpResponse::new(HttpBodyBuilder::new_fake().empty()))
    })
}

#[test]
fn matching_body_is_log_only_field() {
    let (body, capture) = respond(
        logging(&["application/json"]),
        Echoing(StatusCode::OK),
        request("application/json", scripted([data(r#"{"stars":5}"#)])),
    );

    let frames = read_to_end(body);

    let dimensions = vec![
        ("http.request.method", Value::from("POST")),
        ("network.protocol.version", Value::from("1.1")),
        ("http.response.status_code", Value::I64(200)),
    ];
    let mut logs = dimensions.clone();
    logs.extend([(DURATION, Value::F64(0.0)), (BODY, Value::from(r#"{"stars":5}"#))]);
    assert_eq!(
        (frames, capture.single()),
        (
            vec![r#"{"stars":5}"#.to_owned()],
            Captured {
                name: COMPLETE,
                severity: Some(Severity::Info),
                message: Some("HTTP server request completed".into()),
                logs,
                dimensions,
                measurements: vec![duration(0.0)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn body_is_logged_only_for_covered_type() {
    let logged = |types: &[&'static str], content_type: &'static str, text: &str| {
        let request = request(content_type, HttpBodyBuilder::new_fake().text(text));
        let (_body, capture) = respond(logging(types), common::responding(StatusCode::OK), request);
        capture.single().log(BODY).cloned()
    };

    assert_eq!(
        [
            logged(&["application/json"], "application/json", "chunk"),
            logged(&["application/json"], "image/png", "chunk"),
            logged(&[], "application/json", "chunk"),
            logged(&["application/json"], "application/json", ""),
        ],
        [Some(Value::from("chunk")), None, None, Some(Value::from(""))]
    );
}

#[test]
fn unread_body_logs_empty_text() {
    let (_body, capture) = respond(
        logging(&["text/plain"]),
        common::responding(StatusCode::OK),
        request("text/plain", scripted([data("hello")])),
    );

    assert_eq!(capture.single().log(BODY).cloned(), Some(Value::from("")));
}

#[test]
fn body_read_before_middleware_logs_empty_text() {
    let mut body = HttpBodyBuilder::new_fake().text("hello");
    let _ = poll_frame(&mut body);

    let (_body, capture) = respond(
        logging(&["text/plain"]),
        common::responding(StatusCode::OK),
        request("text/plain", body),
    );

    assert_eq!(capture.single().log(BODY).cloned(), Some(Value::from("")));
}

#[test]
fn invalid_media_type_returns_error() {
    let error = Telemetry::server(Sink::noop()).request_body_media_types(["json"]).unwrap_err();

    assert_eq!(error.message(), "'json' is not a media type");
}

#[test]
fn in_memory_body_stays_in_memory() {
    let kept = Arc::default();
    let (_body, capture) = respond(
        logging(&["text/plain"]),
        keeping(&kept),
        request("text/plain", HttpBodyBuilder::new_fake().text("chunk")),
    );

    let body = kept.lock().unwrap().take().unwrap();

    assert_eq!(
        (BytesView::try_from(body).unwrap().to_vec(), capture.single().log(BODY).cloned()),
        (b"chunk".to_vec(), Some(Value::from("chunk")))
    );
}

#[test]
fn handler_receives_every_frame_and_error_in_order() {
    let received = |steps: Vec<Step>| {
        let (body, capture) = respond(
            |layer| logging(&["text/plain"])(layer).body_size_limit(5),
            Echoing(StatusCode::OK),
            request("text/plain", scripted(steps)),
        );
        (read_to_end(body), capture.single().log(BODY).cloned())
    };

    assert_eq!(
        [
            received(vec![data("he"), data("llo"), data(" world"), Step::Trailers]),
            received(vec![
                data("he"),
                Step::Error(HttpError::validation("the review is cut")),
                data("llo")
            ]),
        ],
        [
            (
                vec!["he".to_owned(), "llo".to_owned(), " world".to_owned(), "trailers".to_owned()],
                Some(Value::from("hello"))
            ),
            (
                vec!["he".to_owned(), "validation".to_owned(), "llo".to_owned()],
                Some(Value::from("he"))
            ),
        ]
    );
}

#[test]
fn size_hint_and_body_options_are_kept() {
    let clock = ClockControl::new();
    let bodies = HttpBodyBuilder::new(GlobalPool::new(), &clock.to_clock())
        .with_options(HttpBodyOptions::new().timeout(Duration::from_secs(10)).buffer_limit(8));
    let chunk = |text: &str| Ok::<_, HttpError>(BytesView::copied_from_slice(text.as_bytes(), &bodies));
    let kept = Arc::default();
    let send = |body: HttpBody| {
        let _response = respond(logging(&["text/plain"]), keeping(&kept), request("text/plain", body));
        kept.lock().unwrap().take().unwrap()
    };

    let sized = send(common::sized_scripted([data("hello"), data(" world")])).size_hint().exact();
    let buffered = send(bodies.stream(stream::iter([chunk("hello"), chunk(" world")])))
        .into_bytes()
        .unwrap_ready()
        .unwrap_err();
    let mut waiting = send(bodies.stream(stream::iter([chunk("hel")]).chain(stream::pending())));
    let first = read_frame(&mut waiting);
    let pending = read_frame(&mut waiting);
    clock.advance(Duration::from_secs(10));
    let after_timeout = read_frame(&mut waiting);

    assert_eq!(
        (sized, buffered.label().as_str(), first, pending, after_timeout),
        (
            Some(11),
            "body_size_limit",
            "hel".to_owned(),
            "Pending".to_owned(),
            "body_timeout".to_owned()
        )
    );
}

/// Polls `body` once, and describes the frame as [`read_to_end`] does, or the poll when it has no frame.
fn read_frame(body: &mut HttpBody) -> String {
    match poll_frame(body) {
        Poll::Ready(Some(Ok(frame))) => String::from_utf8_lossy(&frame.into_data().unwrap().to_vec()).into_owned(),
        Poll::Ready(Some(Err(error))) => error.label().to_string(),
        poll => format!("{poll:?}"),
    }
}

#[test]
fn events_log_bytes_that_handler_read() {
    let logged = |last: Step| {
        let (sink, capture) = common::capture_sink();
        let service = logging(&["text/plain"])(server(sink)).layer(reading());
        let mut response = Box::pin(service.execute(request("text/plain", scripted([data("hel"), last]))));
        let resumed = catch_unwind(AssertUnwindSafe(|| poll_once(&mut response))).is_err();
        drop(response);
        let event = capture.single();
        (resumed, event.name, event.log("error.type").cloned(), event.log(BODY).cloned())
    };

    assert_eq!(
        [logged(Step::End), logged(Step::Panic), logged(Step::Pending)],
        [
            (false, COMPLETE, None, Some(Value::from("hel"))),
            (true, ERROR, Some(Value::from("panic")), Some(Value::from("hel"))),
            (false, ABANDONED, Some(Value::from("abandoned")), Some(Value::from("hel")))
        ]
    );
}
