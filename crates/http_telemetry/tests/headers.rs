// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The header fields of the server and client events.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::task::Poll;

use common::{
    ABANDONED, CLIENT_ABANDONED, CLIENT_COMPLETE, CLIENT_ERROR, COMPLETE, Capture, Captured, ERROR, Echoing, PUBLIC, REQUEST_ID, Step,
    StopAt, TOTAL_ABANDONED, TOTAL_ERROR, client, poll_frame, scripted, server,
};
use data_privacy::RedactionEngine;
use future_assert::FutureTestExt;
use http::header::RETRY_AFTER;
use http::{HeaderName, HeaderValue, StatusCode};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use layered::{Execute, Layer, Service};
use observed::{Sink, SinkId, Value};
use tick::SimpleClock;

const TAG: HeaderName = HeaderName::from_static("x-tag");

fn get(headers: &[(HeaderName, &'static [u8])]) -> HttpRequest {
    let mut request = common::get_to("http://example.com/books/1");
    for (name, value) in headers {
        request.headers_mut().append(name, HeaderValue::from_bytes(value).unwrap());
    }
    request
}

/// The log fields `http.request.header.x-request-id` and `http.response.header.retry-after` of `event`.
fn header_logs(event: &Captured) -> [Option<Value>; 2] {
    ["http.request.header.x-request-id", "http.response.header.retry-after"].map(|key| event.log(key).cloned())
}

#[test]
fn configured_headers_are_log_fields_with_joined_values() {
    let (sink, capture) = common::capture_sink();
    let utf8 = HeaderName::from_static("x-utf8");
    let request_classes = [
        (REQUEST_ID, PUBLIC),
        (TAG, PUBLIC),
        (HeaderName::from_static("x-absent"), PUBLIC),
        (utf8.clone(), PUBLIC),
    ];
    let services = [
        server(sink.clone())
            .request_header_data_classes(request_classes.clone())
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
        client(sink)
            .request_header_data_classes(request_classes)
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
    ];

    for service in services {
        service
            .execute(get(&[
                (REQUEST_ID, b"7"),
                (TAG, b"a"),
                (TAG, b"caf\xe9"),
                (TAG, b"b"),
                (utf8.clone(), "café".as_bytes()),
                (RETRY_AFTER, b"120"),
            ]))
            .unwrap_ready()
            .unwrap();
    }

    // The header fields of each event: its log fields, sorted by key, and its metric dimensions.
    let header_fields = |event: &Captured| {
        [&event.logs, &event.dimensions].map(|fields| {
            common::sorted(fields)
                .into_iter()
                .filter(|(key, _)| key.contains(".header."))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let logs = vec![
        ("http.request.header.x-request-id", Value::from("7")),
        ("http.request.header.x-tag", Value::from("a, caf\u{fffd}, b")),
        ("http.request.header.x-utf8", Value::from("café")),
        ("http.response.header.retry-after", Value::from("120")),
    ];
    let events: Vec<_> = capture.events().iter().map(|event| (event.name, header_fields(event))).collect();
    assert_eq!(
        events,
        [COMPLETE, CLIENT_COMPLETE].map(|name| (name, [logs.clone(), vec![]])).to_vec()
    );
}

#[test]
fn header_values_are_taken_when_request_starts_and_when_handler_returns() {
    let capture = Capture::new(|name| name == COMPLETE || name == CLIENT_COMPLETE);
    let handler = || {
        let bodies = HttpBodyBuilder::new_fake();
        Execute::new(move |mut request: HttpRequest| {
            request.headers_mut().insert(REQUEST_ID, HeaderValue::from_static("handler"));
            let mut response = HttpResponse::new(bodies.empty());
            response.headers_mut().insert(REQUEST_ID, HeaderValue::from_static("response"));
            async move { Ok::<_, HttpError>(response) }
        })
    };
    let services = [
        server(common::sink_with(&capture))
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(REQUEST_ID, PUBLIC)])
            .layer(handler()),
        client(common::sink_with(&capture))
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(REQUEST_ID, PUBLIC)])
            .layer(handler()),
    ];

    for service in services {
        service.execute(get(&[(REQUEST_ID, b"request")])).unwrap_ready().unwrap();
    }

    let events: Vec<_> = capture
        .events()
        .iter()
        .map(|event| {
            (
                event.name,
                ["http.request.header.x-request-id", "http.response.header.x-request-id"].map(|key| event.log(key).cloned()),
            )
        })
        .collect();
    assert_eq!(
        events,
        [COMPLETE, CLIENT_COMPLETE]
            .map(|name| (name, [Some(Value::from("request")), Some(Value::from("response"))]))
            .to_vec()
    );
}

#[test]
fn request_or_response_headers_alone_are_logged() {
    let (sink, capture) = common::capture_sink();
    let services = [
        server(sink.clone())
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
        server(sink.clone())
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
        client(sink.clone())
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
        client(sink)
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
    ];

    for service in services {
        service
            .execute(get(&[(REQUEST_ID, b"7"), (RETRY_AFTER, b"120")]))
            .unwrap_ready()
            .unwrap();
    }

    let events: Vec<_> = capture.events().iter().map(|event| (event.name, header_logs(event))).collect();
    let (request, response) = (Some(Value::from("7")), Some(Value::from("120")));
    assert_eq!(
        events,
        [
            (COMPLETE, [request.clone(), None]),
            (COMPLETE, [None, response.clone()]),
            (CLIENT_COMPLETE, [request, None]),
            (CLIENT_COMPLETE, [None, response]),
        ]
    );
}

#[test]
fn error_and_abandoned_events_have_header_fields() {
    let headers = |steps: Vec<Step>, finish: fn(HttpBody)| {
        let capture = Capture::new(|name| name == ERROR || name == ABANDONED);
        let body = server(common::sink_with(&capture))
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK))
            .execute(get(&[(REQUEST_ID, b"7"), (RETRY_AFTER, b"120")]).map(|_| scripted(steps)))
            .unwrap_ready()
            .unwrap()
            .into_body();
        finish(body);
        let event = capture.single();
        (event.name, header_logs(&event))
    };

    let fields = [Some(Value::from("7")), Some(Value::from("120"))];
    assert_eq!(
        [
            headers(vec![Step::Error(HttpError::validation("the page is missing"))], |mut body| {
                let _ = poll_frame(&mut body);
            }),
            headers(vec![Step::Panic], |mut body| {
                catch_unwind(AssertUnwindSafe(|| poll_frame(&mut body))).unwrap_err();
            }),
            headers(vec![Step::Pending], drop),
        ],
        [(ERROR, fields.clone()), (ERROR, fields.clone()), (ABANDONED, fields)]
    );
}

#[test]
fn client_error_and_abandoned_events_have_request_headers() {
    let (sink, capture) = common::capture_sink();
    let panicking =
        || Execute::new(|_: HttpRequest| poll_fn(|_| -> Poll<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") }));

    for report_total_duration in [false, true] {
        let layer = client(sink.clone())
            .report_total_duration(report_total_duration)
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)]);
        let request = || get(&[(REQUEST_ID, b"7")]);
        layer.layer(common::FAILING).execute(request()).unwrap_ready().unwrap_err();
        catch_unwind(AssertUnwindSafe(|| layer.layer(panicking()).execute(request()).unwrap_ready())).unwrap_err();
        drop(layer.layer(common::NeverResponding).execute(request()));
    }

    let events: Vec<_> = capture.events().iter().map(|event| (event.name, header_logs(event))).collect();
    assert_eq!(
        events,
        [
            CLIENT_ERROR,
            CLIENT_ERROR,
            CLIENT_ABANDONED,
            TOTAL_ERROR,
            TOTAL_ERROR,
            TOTAL_ABANDONED
        ]
        .map(|name| (name, [Some(Value::from("7")), None]))
        .to_vec()
    );
}

#[test]
fn each_processor_redacts_headers_with_its_own_redactor() {
    let interested = |name: &str| name == COMPLETE || name == CLIENT_COMPLETE;
    let printing = Capture::new(interested);
    let erasing = Capture::with_engine(interested, RedactionEngine::default());
    let sink = Sink::builder(SinkId::new("two"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(printing.clone()), Arc::new(erasing.clone())])
        .build();
    let services = [
        server(sink.clone())
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
        client(sink)
            .request_header_data_classes([(REQUEST_ID, PUBLIC)])
            .response_header_data_classes([(RETRY_AFTER, PUBLIC)])
            .layer(Echoing(StatusCode::OK)),
    ];

    for service in services {
        service
            .execute(get(&[(REQUEST_ID, b"7"), (RETRY_AFTER, b"120")]))
            .unwrap_ready()
            .unwrap();
    }

    let headers = |capture: Capture| -> Vec<_> { capture.events().iter().map(|event| (event.name, header_logs(event))).collect() };
    let printed = [Some(Value::from("7")), Some(Value::from("120"))];
    let erased = [Some(Value::from("")), Some(Value::from(""))];
    assert_eq!(
        [printing, erasing].map(headers),
        [
            vec![(COMPLETE, printed.clone()), (CLIENT_COMPLETE, printed)],
            vec![(COMPLETE, erased.clone()), (CLIENT_COMPLETE, erased)]
        ]
    );
}

#[test]
fn processor_that_stops_at_header_visits_no_later_field() {
    let stop = StopAt::new("http.request.header.");
    server(common::stop_sink(&stop))
        .request_header_data_classes([(REQUEST_ID, PUBLIC), (TAG, PUBLIC)])
        .response_header_data_classes([(REQUEST_ID, PUBLIC)])
        .layer(Echoing(StatusCode::OK))
        .execute(get(&[(REQUEST_ID, b"7"), (TAG, b"a")]))
        .unwrap_ready()
        .unwrap();

    assert_eq!(stop.after_stop(), Some(vec![]));
}
