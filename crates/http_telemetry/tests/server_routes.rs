// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The route fields of the server events.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::borrow::Cow;
use std::sync::Arc;

use common::{COMPLETE, Capture, Captured, DURATION, PUBLIC, duration, server};
use data_privacy::RedactionEngine;
use future_assert::FutureTestExt;
use http::{Method, StatusCode};
use http_extensions::HttpRequest;
use http_telemetry::TelemetryLayer;
use layered::{Layer, Service};
use observed::{Severity, Sink, SinkId, Value};
use routerama::{RouteHit, RouteTable};
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
}

fn routed(sink: Sink) -> TelemetryLayer {
    server(sink).route_resolver(Arc::new(Route::resolver()))
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

#[routerama::resolver]
enum Books {
    #[route(GET, "/books")]
    List,
}

/// A route table that matches as the resolver of [`Books`] does, but that reports `count` routes, and `template` and
/// `parameters` for every route number.
struct Inconsistent {
    count: usize,
    template: Option<&'static str>,
    parameters: Option<&'static [Cow<'static, str>]>,
}

impl RouteTable for Inconsistent {
    fn route_count(&self) -> usize {
        self.count
    }

    fn template(&self, _route: usize) -> Option<&str> {
        self.template
    }

    fn parameters(&self, _route: usize) -> Option<&[Cow<'static, str>]> {
        self.parameters
    }

    fn match_route<'p>(&self, method: &str, path: &'p str) -> Option<RouteHit<'p>> {
        Books::resolver().match_route(method, path)
    }
}

#[test]
fn match_that_table_cannot_describe_has_no_route() {
    let route = |table: Inconsistent| {
        let capture = Capture::new(|name| name == COMPLETE);
        server(common::sink_with(&capture))
            .route_resolver(Arc::new(table))
            .layer(common::responding(StatusCode::OK))
            .execute(common::get_to("/books"))
            .unwrap_ready()
            .unwrap();
        capture.single().log("http.route").cloned()
    };

    assert_eq!(
        [
            route(Inconsistent {
                count: 1,
                template: None,
                parameters: Some(&[]),
            }),
            route(Inconsistent {
                count: 1,
                template: Some("/books"),
                parameters: None,
            }),
            route(Inconsistent {
                count: 0,
                template: Some("/books"),
                parameters: Some(&[]),
            }),
        ],
        [None, None, None]
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
