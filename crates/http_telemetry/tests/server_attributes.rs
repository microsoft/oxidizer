// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `server.address` and `server.port` of the server events, and the requests that
//! `excluded_paths` leaves out.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::task::Poll;

use common::{COMPLETE, Capture, Echoing, Step, data, poll_frame, scripted, server};
use future_assert::FutureTestExt;
use http::header::HOST;
use http::{HeaderValue, StatusCode};
use http_extensions::HttpRequest;
use http_telemetry::TelemetryLayer;
use layered::{Layer, Service};
use observed::Value;

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

/// Whether the server layer that `configure` sets up leaves out the request to `uri`, so that it
/// emits no event.
fn is_excluded(configure: impl FnOnce(TelemetryLayer) -> TelemetryLayer, uri: &'static str) -> bool {
    let capture = Capture::new(|_| true);
    configure(server(common::sink_with(&capture)))
        .layer(common::responding(StatusCode::OK))
        .execute(get(uri, None))
        .unwrap_ready()
        .unwrap();
    capture.names().is_empty()
}

#[test]
fn server_address_and_port_come_from_uri_authority_else_host_header() {
    let rows = [
        ("http://example.com:8080/", Some("other:9090"), Some("example.com"), Some(8080)),
        ("https://example.com/", Some("other:9090"), Some("example.com"), Some(443)),
        ("http://example.com/", None, Some("example.com"), Some(80)),
        ("ftp://example.com/", None, Some("example.com"), None),
        ("http://[2001:db8::1]:8443/", None, Some("[2001:db8::1]"), Some(8443)),
        ("/", Some("example.com:8080"), Some("example.com"), Some(8080)),
        ("/", Some("[2001:db8::1]:8443"), Some("[2001:db8::1]"), Some(8443)),
        ("/", Some("example.com"), Some("example.com"), None),
        ("/", Some("example.com:"), Some("example.com"), None),
        ("/", Some("exa mple.com"), None, None),
        ("/", Some(""), None, None),
        ("/", None, None, None),
    ];
    let fields = |address: Option<&'static str>, port: Option<i64>| {
        let address = address.map(Value::from);
        let port = port.map(Value::I64);
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
        .excluded_paths(["/health"])
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
fn excluded_paths_match_raw_path_start_ignoring_ascii_case() {
    let rows: [(&[&str], &'static str, bool); 10] = [
        (&["/health"], "/health", true),
        (&["/health"], "/HEALTH/live", true),
        (&["/Health"], "/healthz?probe=1", true),
        (&["/metrics", "/health"], "/health", true),
        (&[""], "/books/1", true),
        (&["/health"], "/api/health", false),
        (&["/health"], "/%68ealth", false),
        (&["/health?"], "/health?probe=1", false),
        (&["/health"], "/hea", false),
        (&[], "/health", false),
    ];

    assert_eq!(
        rows.map(|(paths, uri, _)| is_excluded(|layer| layer.excluded_paths(paths.iter().copied()), uri)),
        rows.map(|(.., excluded)| excluded)
    );
}

#[test]
fn excluded_paths_replace_earlier_paths() {
    let configure = |layer: TelemetryLayer| layer.excluded_paths(["/health"]).excluded_paths(["/metrics"]);

    assert_eq!(
        [is_excluded(configure, "/metrics"), is_excluded(configure, "/health")],
        [true, false]
    );
}
