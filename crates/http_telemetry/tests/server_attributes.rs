// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `server.address` and `server.port` of the server events, and the requests that
//! `excluded_paths` leaves out.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::task::Poll;

use common::{ACTIVE, COMPLETE, Capture, Echoing, Step, data, poll_frame, scripted, server};
use future_assert::FutureTestExt;
use http::header::HOST;
use http::{HeaderValue, StatusCode};
use http_extensions::HttpRequest;
use http_telemetry::TelemetryLayer;
use layered::{Layer, Service};
use observed::Value;
use templated_uri::BasePath;

/// A `GET` request to `uri`, with the `Host` header `host` if it is `Some`.
fn get(uri: &'static str, host: Option<&'static str>) -> HttpRequest {
    let mut request = common::get_to(uri);
    if let Some(host) = host {
        request.headers_mut().insert(HOST, HeaderValue::from_static(host));
    }
    request
}

/// The `server.address` and the `server.port` of the complete event of `request`, each as a log
/// field and as a metric dimension.
fn server_fields(request: HttpRequest) -> [(Option<Value>, Option<Value>); 2] {
    let capture = Capture::new(|name| name == COMPLETE);
    server(common::sink_with(&capture))
        .layer(common::responding(StatusCode::OK))
        .execute(request)
        .unwrap_ready()
        .unwrap();
    let event = capture.single();
    ["server.address", "server.port"].map(|key| (event.log(key).cloned(), event.dimension(key).cloned()))
}

/// The names of the events that the server layer that `configure` sets up emits for a request to
/// `uri`.
fn event_names(configure: impl FnOnce(TelemetryLayer) -> TelemetryLayer, uri: &'static str) -> Vec<&'static str> {
    let capture = Capture::new(|_| true);
    configure(server(common::sink_with(&capture)))
        .layer(common::responding(StatusCode::OK))
        .execute(get(uri, None))
        .unwrap_ready()
        .unwrap();
    capture.names()
}

#[test]
fn server_address_and_port_come_from_uri_authority_else_host_header() {
    let rows = [
        ("http://example.com:8080/", Some("other:9090"), "example.com", 8080),
        ("https://example.com/", Some("other:9090"), "example.com", 443),
        ("http://example.com/", None, "example.com", 80),
        ("/", Some("example.com:8080"), "example.com", 8080),
    ];
    let fields = |address: &'static str, port: i64| {
        let (address, port) = (Some(Value::from(address)), Some(Value::I64(port)));
        [(address.clone(), address), (port.clone(), port)]
    };

    assert_eq!(
        rows.map(|(uri, host, ..)| server_fields(get(uri, host))),
        rows.map(|(.., address, port)| fields(address, port))
    );
}

#[test]
fn excluded_request_reaches_handler_unchanged_and_emits_no_event() {
    let capture = Capture::new(|_| true);
    let service = server(common::sink_with(&capture))
        .excluded_paths([BasePath::from_static("/health/")])
        .layer(Echoing(StatusCode::ACCEPTED));

    let response = service
        .execute(get("/health/live", None).map(|_| scripted([data("ok"), Step::Pending])))
        .unwrap_ready()
        .unwrap();
    let status = response.status();
    let mut body = response.into_body();
    let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
        panic!("the body of the handler must return its first frame");
    };
    // A wrapped body that is dropped before its end would emit the abandoned event.
    drop(body);

    assert_eq!(
        (status, frame.into_data().unwrap().to_vec(), capture.names()),
        (StatusCode::ACCEPTED, b"ok".to_vec(), vec![])
    );
}

#[test]
fn excluded_paths_match_raw_path_and_subpaths_ignoring_ascii_case() {
    let rows: [(&[&'static str], &'static str, bool); 9] = [
        (&["/health/"], "/health", true),
        (&["/health/"], "/HEALTH/live", true),
        (&["/Health/"], "/health?probe=1", true),
        (&["/metrics/", "/health/"], "/health", true),
        (&["/"], "/books/1", true),
        (&["/health/"], "/healths", false),
        (&["/health/"], "/api/health", false),
        (&["/health/"], "/%68ealth", false),
        (&["/health/"], "/hea", false),
    ];
    let excluded = |paths: &[&'static str], uri| {
        let configure = |layer: TelemetryLayer| layer.excluded_paths(paths.iter().map(|path| BasePath::from_static(path)));
        event_names(configure, uri).is_empty()
    };

    assert_eq!(
        rows.map(|(paths, uri, _)| excluded(paths, uri)),
        rows.map(|(.., excluded)| excluded)
    );
}

#[test]
fn excluded_paths_replace_earlier_paths() {
    let configure = |layer: TelemetryLayer| {
        layer
            .excluded_paths([BasePath::from_static("/health/")])
            .excluded_paths([BasePath::from_static("/metrics/")])
    };

    assert_eq!(event_names(configure, "/health"), vec![ACTIVE, COMPLETE, ACTIVE]);
}
