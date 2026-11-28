// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The response body that the middleware logs as `http.response.body.content`.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytesbuf::BytesView;
use common::{ABANDONED, CLIENT_COMPLETE, CLIENT_DURATION, Capture, ERROR, Echoing, Step, client, data, poll_frame, scripted, server};
use future_assert::FutureTestExt;
use http::header::{CONTENT_ENCODING, CONTENT_TYPE};
use http::{HeaderValue, Method, StatusCode, Version};
use http_body::{Body, Frame};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::Telemetry;
use layered::{Execute, Layer, Service};
use observed::{Sink, Value};
use ohno::{ErrorExt, Labeled};
use tick::ClockControl;

const BODY: &str = "http.response.body.content";

/// A server layer and a client layer over [`Echoing`] with 200 that log the response bodies of `types`, up to `limit`
/// bytes if set, and a capture of the events of each side, except the deltas of `http.server.active_requests`.
fn sides(types: &[&'static str], limit: Option<usize>) -> [(Telemetry<Echoing>, Capture); 2] {
    let (server_sink, server_capture) = common::capture_sink();
    let (client_sink, client_capture) = common::capture_sink();
    let server_layer = server(server_sink).response_body_media_types(types.iter().copied()).unwrap();
    let client_layer = client(client_sink).response_body_media_types(types.iter().copied()).unwrap();
    let (server_layer, client_layer) = match limit {
        Some(limit) => (server_layer.body_size_limit(limit), client_layer.body_size_limit(limit)),
        None => (server_layer, client_layer),
    };
    [
        (server_layer.layer(Echoing(StatusCode::OK)), server_capture),
        (client_layer.layer(Echoing(StatusCode::OK)), client_capture),
    ]
}

/// Sends `request()` through each of [`sides`], and reads each response body to its end. Returns the log field
/// `http.response.body.content` of the event of each side, the server first.
fn logged_bodies(types: &[&'static str], limit: Option<usize>, request: impl Fn() -> HttpRequest) -> [Option<Value>; 2] {
    sides(types, limit).map(|(service, capture)| {
        read_to_end(service.execute(request()).unwrap_ready().unwrap().into_body());
        capture.single().log(BODY).cloned()
    })
}

/// A request with `method` over HTTP/1.1, with `body`, and with `content_type` as its `Content-Type`. [`Echoing`]
/// responds with both.
fn request(method: Method, content_type: &'static str, body: HttpBody) -> HttpRequest {
    let mut request = common::request(method, Version::HTTP_11).map(|_| body);
    request.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    request
}

/// Polls `body` until it ends or is pending, drops it, and describes each frame: the text of a data frame, `trailers`,
/// or the label of an error.
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

#[test]
fn matching_body_is_log_only_field() {
    let body_fields = sides(&["application/json"], None).map(|(service, capture)| {
        let request = request(Method::GET, "application/json", HttpBodyBuilder::new_fake().text(r#"{"book":1}"#));
        read_to_end(service.execute(request).unwrap_ready().unwrap().into_body());
        let event = capture.single();
        [&event.logs, &event.dimensions].map(|fields| fields.iter().filter(|(key, _)| key.contains(".body.")).cloned().collect::<Vec<_>>())
    });

    let fields = [vec![(BODY, Value::from(r#"{"book":1}"#))], vec![]];
    assert_eq!(body_fields, [fields.clone(), fields]);
}

#[test]
fn body_is_logged_when_media_types_cover_content_type() {
    let logged = |types: &[&'static str], content_type: Option<&'static str>| {
        logged_bodies(types, None, || {
            let mut request = common::get().map(|_| HttpBodyBuilder::new_fake().text("chunk"));
            if let Some(content_type) = content_type {
                request
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
            }
            request
        })
    };
    let chunk = || Some(Value::from("chunk"));
    let rows: [(&[&'static str], Option<&'static str>, Option<Value>); 20] = [
        (&["application/json"], Some("application/json"), chunk()),
        (&["image/png", "application/json"], Some("application/json"), chunk()),
        (&["application/json"], Some("image/png"), None),
        (&["video/ogg"], Some("audio/ogg"), None),
        (&["application/json"], None, None),
        (&[], Some("text/plain"), None),
        (&["application/json"], Some("text/csv"), chunk()),
        (&["application/json"], Some("application/xml"), None),
        (&["application/json"], Some("application/problem+json"), chunk()),
        (&["application/json"], Some("application/soap+xml"), chunk()),
        (&["application/json"], Some("application/foo+ld"), None),
        (&["application/json"], Some("image/svg+xml"), None),
        (&["application/json"], Some("Application/JSON ; charset=utf-8"), chunk()),
        (&["application/json"], Some("application/json\t; charset=utf-8"), chunk()),
        (&["APPLICATION/JSON"], Some("application/json"), chunk()),
        (&["image/png"], Some("TEXT/csv"), chunk()),
        (&["image/png"], Some("Application/Problem+JSON"), chunk()),
        (&["application/json; charset=utf-8"], Some("application/json"), chunk()),
        (&["application/json"], Some("not a media type"), None),
        (&["application/json"], Some("application/jsön"), None),
    ];

    assert_eq!(
        rows.clone()
            .map(|(types, content_type, _)| (types, content_type, logged(types, content_type))),
        rows.map(|(types, content_type, body)| (types, content_type, [body.clone(), body]))
    );
}

#[test]
fn body_is_logged_only_without_content_coding() {
    let logged = |lines: &[&'static str]| {
        logged_bodies(&["application/json"], None, || {
            let mut request = request(Method::GET, "application/json", HttpBodyBuilder::new_fake().text("chunk"));
            for line in lines {
                request.headers_mut().append(CONTENT_ENCODING, HeaderValue::from_static(line));
            }
            request
        })
    };
    let chunk = || Some(Value::from("chunk"));

    assert_eq!(
        [
            logged(&["gzip"]),
            logged(&["identity"]),
            logged(&["Identity"]),
            logged(&["identity, gzip"]),
            logged(&["identity", "gzip"]),
            logged(&["identity,\tIDENTITY ,"]),
        ],
        [None, chunk(), chunk(), None, None, chunk()].map(|body| [body.clone(), body])
    );
}

#[test]
fn server_responses_without_content_log_empty_text() {
    let logged = |method: Method, text: &str| {
        let [(service, capture), _] = sides(&["application/json"], None);
        let _response = service
            .execute(request(method, "application/json", HttpBodyBuilder::new_fake().text(text)))
            .unwrap_ready()
            .unwrap();
        capture.single().log(BODY).cloned()
    };

    assert_eq!(
        [logged(Method::GET, ""), logged(Method::HEAD, "chunk")],
        [Some(Value::from("")), Some(Value::from(""))]
    );
}

#[test]
fn logged_text_is_cut_at_limit_and_lossy() {
    fn bytes(slice: &[u8]) -> Step {
        Step::Data(BytesView::copied_from_slice(slice, &HttpBodyBuilder::new_fake()))
    }
    let logged = |limit: usize, steps: fn() -> Vec<Step>| {
        logged_bodies(&["text/plain"], Some(limit), || {
            request(Method::GET, "text/plain", scripted(steps()))
        })
    };

    assert_eq!(
        [
            logged(5, || vec![data("hel"), data("lo wo"), data("rld")]),
            logged(4, || vec![data("café")]),
            logged(5, || vec![data("ab\u{1f600}")]),
            logged(4, || vec![bytes(b"caf\xff")]),
            logged(32, || vec![bytes(b"caf\xe9")]),
        ],
        ["hello", "caf", "ab", "caf\u{fffd}", "caf\u{fffd}"].map(|text| [Some(Value::from(text)), Some(Value::from(text))])
    );
}

#[test]
fn body_of_one_empty_frame_logs_empty_text() {
    let logged = logged_bodies(&["text/plain"], None, || request(Method::GET, "text/plain", scripted([data("")])));

    assert_eq!(logged, [Some(Value::from("")), Some(Value::from(""))]);
}

#[test]
fn server_body_that_reports_end_early_logs_text_up_to_that_end() {
    struct EarlyEnd {
        inner: HttpBody,
        polled: bool,
    }

    impl Body for EarlyEnd {
        type Data = BytesView;
        type Error = HttpError;

        fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<BytesView>, HttpError>>> {
            self.polled = true;
            Pin::new(&mut self.inner).poll_frame(cx)
        }

        fn is_end_stream(&self) -> bool {
            self.polled
        }
    }

    let [(service, capture), _] = sides(&["text/plain"], None);
    let source = HttpBodyBuilder::new_fake().body(EarlyEnd {
        inner: scripted([data("hello"), data(" world")]),
        polled: false,
    });
    let body = service
        .execute(request(Method::GET, "text/plain", source))
        .unwrap_ready()
        .unwrap()
        .into_body();

    assert_eq!(
        (read_to_end(body), capture.single().log(BODY).cloned()),
        (vec!["hello".to_owned(), " world".to_owned()], Some(Value::from("hello")))
    );
}

#[test]
fn default_limit_is_32_kib() {
    let logged = logged_bodies(&["text/plain"], None, || {
        request(Method::GET, "text/plain", HttpBodyBuilder::new_fake().text("a".repeat(32_769)))
    });

    assert_eq!(
        logged,
        [Some(Value::from("a".repeat(32_768))), Some(Value::from("a".repeat(32_768)))]
    );
}

#[test]
fn limit_above_maximum_becomes_1_5_mib() {
    let logged = logged_bodies(&["text/plain"], Some(1_572_865), || {
        request(Method::GET, "text/plain", HttpBodyBuilder::new_fake().text("a".repeat(1_572_865)))
    });

    assert_eq!(
        logged,
        [Some(Value::from("a".repeat(1_572_864))), Some(Value::from("a".repeat(1_572_864)))]
    );
}

#[test]
fn caller_receives_every_frame_of_logged_body() {
    let received = sides(&["text/plain"], Some(5)).map(|(service, capture)| {
        let body = service
            .execute(request(
                Method::GET,
                "text/plain",
                scripted([data("hello"), data(" world"), Step::Trailers]),
            ))
            .unwrap_ready()
            .unwrap()
            .into_body();
        (read_to_end(body), capture.single().log(BODY).cloned())
    });

    let frames = vec!["hello".to_owned(), " world".to_owned(), "trailers".to_owned()];
    assert_eq!(
        received,
        [(frames.clone(), Some(Value::from("hello"))), (frames, Some(Value::from("hello")))]
    );
}

#[test]
fn failed_and_abandoned_bodies_log_bytes_read_before_end() {
    let logged = |steps: fn() -> Vec<Step>| {
        sides(&["text/plain"], None).map(|(service, capture)| {
            let body = service
                .execute(request(Method::GET, "text/plain", scripted(steps())))
                .unwrap_ready()
                .unwrap()
                .into_body();
            let frames = read_to_end(body);
            let event = capture.single();
            (frames, event.name, event.log(BODY).cloned())
        })
    };
    let failed = || vec!["hello".to_owned(), "validation".to_owned()];
    let hello = || Some(Value::from("hello"));

    assert_eq!(
        [
            logged(|| vec![data("hello"), Step::Error(HttpError::validation("the page is missing"))]),
            logged(|| vec![data("hello"), Step::Pending, Step::Pending]),
        ],
        [
            [(failed(), ERROR, hello()), (failed(), CLIENT_COMPLETE, hello())],
            [
                (vec!["hello".to_owned()], ABANDONED, hello()),
                (vec!["hello".to_owned()], CLIENT_COMPLETE, hello())
            ],
        ]
    );
}

#[test]
fn client_event_comes_when_caller_drops_logged_body() {
    let logged = |polls: usize| {
        let [_, (service, capture)] = sides(&["text/plain"], None);
        let mut body = service
            .execute(request(Method::GET, "text/plain", scripted([data("hello"), data(" world")])))
            .unwrap_ready()
            .unwrap()
            .into_body();
        for _ in 0..polls {
            let _ = poll_frame(&mut body);
        }
        let before_drop = capture.names();
        drop(body);
        (before_drop, capture.single().log(BODY).cloned())
    };

    assert_eq!(
        [logged(0), logged(3)],
        [(vec![], Some(Value::from(""))), (vec![], Some(Value::from("hello world")))]
    );
}

#[test]
fn client_event_comes_at_handler_return_when_body_needs_no_copy() {
    let logged = |types: &[&'static str], body: HttpBody| {
        let [_, (service, capture)] = sides(types, None);
        let _response = service.execute(request(Method::GET, "text/plain", body)).unwrap_ready().unwrap();
        capture.single().log(BODY).cloned()
    };

    assert_eq!(
        [
            logged(&[], scripted([data("hello")])),
            logged(&["text/plain"], HttpBodyBuilder::new_fake().text("hello")),
            logged(&["text/plain"], scripted([])),
        ],
        [None, Some(Value::from("hello")), Some(Value::from(""))]
    );
}

#[test]
fn client_duration_ends_when_handler_returns() {
    let clock = ClockControl::new();
    let (sink, capture) = common::capture_sink();
    let handler = {
        let clock = clock.clone();
        Execute::new(move |request: HttpRequest| {
            clock.advance(Duration::from_secs(2));
            let (parts, body) = request.into_parts();
            let mut response = HttpResponse::new(body);
            *response.headers_mut() = parts.headers;
            async move { Ok::<_, HttpError>(response) }
        })
    };
    let service = Telemetry::client(sink)
        .clock(clock.to_simple_clock())
        .response_body_media_types(["text/plain"])
        .unwrap()
        .layer(handler);

    let body = service
        .execute(request(Method::GET, "text/plain", scripted([data("hello")])))
        .unwrap_ready()
        .unwrap()
        .into_body();
    clock.advance(Duration::from_secs(3));
    read_to_end(body);

    assert_eq!(capture.single().log(CLIENT_DURATION).cloned(), Some(Value::F64(2.0)));
}

#[test]
fn client_body_dropped_during_panic_emits_no_event() {
    let [_, (service, capture)] = sides(&["text/plain"], None);
    let body = service
        .execute(request(Method::GET, "text/plain", scripted([Step::Panic])))
        .unwrap_ready()
        .unwrap()
        .into_body();

    let panic = catch_unwind(AssertUnwindSafe(move || {
        let mut body = body;
        poll_frame(&mut body)
    }))
    .unwrap_err();

    assert_eq!((*panic.downcast::<&str>().unwrap(), capture.names()), ("the body panicked", vec![]));
}

#[test]
fn invalid_media_type_returns_error() {
    let messages = |entry: &'static str| {
        [
            Telemetry::server(Sink::noop())
                .response_body_media_types(["application/json", entry])
                .unwrap_err()
                .message(),
            Telemetry::client(Sink::noop())
                .response_body_media_types(["application/json", entry])
                .unwrap_err()
                .message(),
        ]
    };

    assert_eq!(
        ["json", "text/*", "*/*"].map(messages),
        [
            ["'json' is not a media type", "'json' is not a media type"],
            ["'text/*' is not a media type", "'text/*' is not a media type"],
            ["'*/*' is not a media type", "'*/*' is not a media type"]
        ]
    );
}
