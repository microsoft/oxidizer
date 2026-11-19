// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The route fields of the server events.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::future::poll_fn;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::task::Poll;

use common::{ABANDONED, COMPLETE, Capture, Captured, DURATION, ERROR, PUBLIC, StopAt, duration, server};
use data_privacy::RedactionEngine;
use future_assert::FutureTestExt;
use http::{Method, StatusCode};
use http_extensions::{HttpRequest, HttpResponse};
use http_telemetry::TelemetryLayer;
use layered::{Execute, Layer, Service};
use observed::{Severity, Sink, SinkId, Value};
use routerama::HttpMethod;
use tick::SimpleClock;

#[routerama::resolver]
#[expect(
    dead_code,
    reason = "the middleware reads the route table of the resolver, not the captured fields"
)]
enum Route<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: &'p str },
    #[route(PURGE, "/cache/{key}")]
    Purge { key: &'p str },
    /// A route that `routed` adds at run time.
    Plugin { name: String },
}

fn routed(sink: Sink) -> TelemetryLayer {
    let resolver = Route::builder().add_plugin(HttpMethod::GET, "/plugins/{name}").build().unwrap();
    server(sink).route_resolver(resolver)
}

fn request(method: &str, uri: &'static str) -> HttpRequest {
    let mut request = common::get_to(uri);
    *request.method_mut() = Method::from_bytes(method.as_bytes()).unwrap();
    request
}

#[test]
fn matched_route_event_has_route_and_parameters() {
    let (sink, capture) = common::capture_sink();
    routed(sink)
        .route_parameter_data_classes([("book", PUBLIC), ("chapter", PUBLIC)])
        .layer(common::responding(StatusCode::OK))
        .execute(common::get_to("http://example.com/books/rust%20book/reviews/42?sort=new"))
        .unwrap_ready()
        .unwrap();

    let dimensions = vec![
        ("http.request.method", Value::from("GET")),
        ("url.scheme", Value::from("http")),
        ("network.protocol.version", Value::from("1.1")),
        ("server.address", Value::from("example.com")),
        ("server.port", Value::I64(80)),
        ("http.route", Value::from("/books/{book}/reviews/{review}")),
        ("http.response.status_code", Value::I64(200)),
    ];
    let mut logs = dimensions.clone();
    logs.extend([
        (DURATION, Value::F64(0.0)),
        ("http.route.parameter.book", Value::from("rust%20book")),
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
fn http_route_is_template_of_route_that_matches_method_text_and_path() {
    let rows = [
        ("GET", "/books/1", Some("/books/{book}")),
        ("PURGE", "/cache/1", Some("/cache/{key}")),
        ("GET", "/plugins/1", Some("/plugins/{name}")),
        ("GET", "/missing", None),
        ("CONNECT", "example.com:443", None),
    ];
    let route = |method, uri| {
        let capture = Capture::new(|name| name == COMPLETE);
        routed(common::sink_with(&capture))
            .layer(common::responding(StatusCode::OK))
            .execute(request(method, uri))
            .unwrap_ready()
            .unwrap();
        capture.single().log("http.route").cloned()
    };

    assert_eq!(
        rows.map(|(method, uri, _)| route(method, uri)),
        rows.map(|(.., template)| template.map(Value::from))
    );
}

#[test]
fn error_and_abandoned_events_have_route_and_parameters() {
    let layer_for = |capture: &Capture| routed(common::sink_with(capture)).route_parameter_data_classes([("book", PUBLIC)]);

    let failed = Capture::new(|name| name == ERROR);
    layer_for(&failed)
        .layer(common::FAILING)
        .execute(common::get_to("/books/1"))
        .unwrap_ready()
        .unwrap_err();
    let panicked = Capture::new(|name| name == ERROR);
    let panicking =
        Execute::new(|_: HttpRequest| poll_fn(|_| -> Poll<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") }));
    let service = layer_for(&panicked).layer(panicking);
    catch_unwind(AssertUnwindSafe(|| service.execute(common::get_to("/books/1")).unwrap_ready())).unwrap_err();
    let abandoned = Capture::new(|name| name == ABANDONED);
    drop(
        layer_for(&abandoned)
            .layer(common::NeverResponding)
            .execute(common::get_to("/books/1")),
    );

    let route = |capture: Capture| {
        let event = capture.single();
        (
            event.name,
            ["http.route", "http.route.parameter.book"].map(|key| event.log(key).cloned()),
        )
    };
    let fields = [Some(Value::from("/books/{book}")), Some(Value::from("1"))];
    assert_eq!(
        [failed, panicked, abandoned].map(route),
        [(ERROR, fields.clone()), (ERROR, fields.clone()), (ABANDONED, fields)]
    );
}

#[test]
fn each_processor_redacts_parameter_with_its_own_redactor() {
    let printing = Capture::new(|name| name == COMPLETE);
    let erasing = Capture::with_engine(|name| name == COMPLETE, RedactionEngine::default());
    let sink = Sink::builder(SinkId::new("two"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(printing.clone()), Arc::new(erasing.clone())])
        .build();

    routed(sink)
        .route_parameter_data_classes([("book", PUBLIC)])
        .layer(common::responding(StatusCode::OK))
        .execute(common::get_to("/books/rust"))
        .unwrap_ready()
        .unwrap();

    assert_eq!(
        [printing, erasing].map(|capture| capture.single().log("http.route.parameter.book").cloned()),
        [Some(Value::from("rust")), Some(Value::from(""))]
    );
}

#[test]
fn processor_that_stops_at_parameter_visits_no_later_field() {
    let stop = StopAt::new("http.route.parameter.");
    routed(common::stop_sink(&stop))
        .route_parameter_data_classes([("book", PUBLIC), ("review", PUBLIC)])
        .layer(common::responding(StatusCode::OK))
        .execute(common::get_to("/books/rust/reviews/42"))
        .unwrap_ready()
        .unwrap();

    assert_eq!(stop.after_stop(), Some(vec![]));
}
