// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server middleware. The tests cover when the middleware emits each event,
//! their fields and metric, and the effect of the layer configuration.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::time::Duration;

use common::{
    ABANDONED, ACTIVE, COMPLETE, Capture, Captured, DURATION, Echoing, Probe, Step, Tenant, data, duration, poll_frame, scripted, server,
    sorted,
};
use future_assert::{FutureTestExt, poll_once};
use http::{Method, StatusCode, Version};
use layered::{Layer, Service};
use observed::enrichment::EnrichFnExt;
use observed::{Severity, Value};
use tick::ClockControl;

#[test]
fn server_complete_event_reports_method_scheme_status_version_and_duration() {
    let (sink, capture) = common::capture_sink();
    let clock = ClockControl::new();
    let service =
        server(sink)
            .clock(clock.to_simple_clock())
            .layer(common::responding_after(&clock, Duration::from_millis(125), StatusCode::OK));

    let response = service
        .execute(common::get_to("https://example.com/books/1"))
        .unwrap_ready()
        .unwrap();

    assert_eq!(
        (response.status(), capture.single()),
        (
            StatusCode::OK,
            Captured {
                name: COMPLETE,
                severity: Some(Severity::Info),
                message: Some("HTTP server request completed".into()),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("http.response.status_code", Value::I64(200)),
                    (DURATION, Value::F64(0.125)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("http.response.status_code", Value::I64(200)),
                ],
                measurements: vec![duration(0.125)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn server_dropped_future_reports_abandoned_without_status() {
    let capture = Capture::new(|name| name == ABANDONED);
    let clock = ClockControl::new();
    let service = server(common::sink_with(&capture))
        .clock(clock.to_simple_clock())
        .layer(common::NeverResponding);

    let mut future = Box::pin(service.execute(common::get_to("https://example.com/books/1")));
    assert!(poll_once(&mut future).is_pending());
    clock.advance(Duration::from_millis(40));
    drop(future);

    assert_eq!(
        capture.single(),
        Captured {
            name: ABANDONED,
            severity: Some(Severity::Warn),
            message: Some("HTTP server request abandoned".into()),
            logs: vec![
                ("http.request.method", Value::from("GET")),
                ("url.scheme", Value::from("https")),
                ("network.protocol.version", Value::from("1.1")),
                ("server.address", Value::from("example.com")),
                ("server.port", Value::I64(443)),
                ("error.type", Value::from("abandoned")),
                (DURATION, Value::F64(0.04)),
            ],
            dimensions: vec![
                ("http.request.method", Value::from("GET")),
                ("url.scheme", Value::from("https")),
                ("network.protocol.version", Value::from("1.1")),
                ("server.address", Value::from("example.com")),
                ("server.port", Value::I64(443)),
                ("error.type", Value::from("abandoned")),
            ],
            measurements: vec![duration(0.04)],
            enrichments: vec![],
        }
    );
}

#[test]
fn url_scheme_is_http_https_or_other() {
    let scheme_of = |uri: &'static str| {
        let capture = Capture::new(|name| name == COMPLETE);
        let service = server(common::sink_with(&capture)).layer(common::responding(StatusCode::OK));
        service.execute(common::get_to(uri)).unwrap_ready().unwrap();
        let event = capture.single();
        (event.log("url.scheme").cloned(), event.dimension("url.scheme").cloned())
    };

    assert_eq!(
        [
            "http://example.com/books/1",
            "https://example.com/books/1",
            "ftp://example.com/books/1"
        ]
        .map(scheme_of),
        ["http", "https", "_OTHER"].map(|scheme| (Some(Value::from(scheme)), Some(Value::from(scheme))))
    );
}

#[test]
fn known_methods_keep_their_names() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::responding(StatusCode::OK));
    let names = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"];

    for name in names {
        let method = Method::from_bytes(name.as_bytes()).unwrap();
        service.execute(common::request(method, Version::HTTP_11)).unwrap_ready().unwrap();
    }

    let methods: Vec<_> = capture
        .events()
        .iter()
        .map(|event| event.log("http.request.method").cloned())
        .collect();
    assert_eq!(methods, names.map(|name| Some(Value::from(name))).to_vec());
}

#[test]
fn unknown_method_meters_other_and_logs_original() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::responding(StatusCode::OK));

    service
        .execute(common::request(Method::from_bytes(b"PURGE").unwrap(), Version::HTTP_11))
        .unwrap_ready()
        .unwrap();

    let event = capture.single();
    assert_eq!(
        (sorted(&event.logs), sorted(&event.dimensions)),
        (
            sorted(&[
                ("http.request.method", Value::from("_OTHER")),
                ("http.request.method_original", Value::from("PURGE")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(200)),
                (DURATION, Value::F64(0.0)),
            ]),
            sorted(&[
                ("http.request.method", Value::from("_OTHER")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(200)),
            ]),
        )
    );
}

#[test]
fn protocol_version_uses_open_telemetry_values() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::responding(StatusCode::OK));

    for version in [
        Version::HTTP_09,
        Version::HTTP_10,
        Version::HTTP_11,
        Version::HTTP_2,
        Version::HTTP_3,
    ] {
        service.execute(common::request(Method::GET, version)).unwrap_ready().unwrap();
    }

    let versions: Vec<_> = capture
        .events()
        .iter()
        .map(|event| event.log("network.protocol.version").cloned())
        .collect();
    assert_eq!(
        versions,
        ["_OTHER", "1.0", "1.1", "2", "3"]
            .map(|version| Some(Value::from(version)))
            .to_vec()
    );
}

#[test]
fn start_scope_enrichment_reaches_server_events() {
    let capture = Capture::new(|_| true);
    let sink = common::sink_with(&capture);
    let completing = server(sink.clone()).layer(common::responding(StatusCode::OK));
    let abandoning = server(sink.clone()).layer(common::NeverResponding);
    let streaming = server(sink.clone()).layer(Echoing(StatusCode::OK));
    let streamed_request = common::get().map(|_| scripted([data("chunk"), Step::End]));

    let (complete, abandoned, streamed) = (|| {
        (
            completing.execute(common::get()),
            abandoning.execute(common::get()),
            streaming.execute(streamed_request),
        )
    })
    .enrich(&sink, Tenant { tenant: "contoso" })();
    complete.unwrap_ready().unwrap();
    drop(abandoned);
    // The guard moves into the response body, which ends after the scope.
    let mut body = streamed.unwrap_ready().unwrap().into_body();
    let _ = poll_frame(&mut body);
    let _ = poll_frame(&mut body);

    let enrichments: Vec<_> = capture.events().into_iter().map(|event| (event.name, event.enrichments)).collect();
    let tenant = vec![("tenant", Value::from("contoso"))];
    assert_eq!(
        enrichments,
        [ACTIVE, ACTIVE, ACTIVE, COMPLETE, ACTIVE, ABANDONED, ACTIVE, COMPLETE, ACTIVE]
            .map(|name| (name, tenant.clone()))
            .to_vec()
    );
}

#[test]
fn interest_is_checked_for_each_request() {
    let probe = Probe::default();
    let service = server(common::probe_sink(&probe)).layer(common::responding(StatusCode::IM_A_TEAPOT));

    let uninterested = service.execute(common::get()).unwrap_ready().unwrap();
    probe.set_interested(true);
    service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!(
        (uninterested.status(), probe.received()),
        (StatusCode::IM_A_TEAPOT, vec![ACTIVE, COMPLETE, ACTIVE])
    );
}

#[test]
fn layer_and_middleware_debug_output_names_their_types() {
    let (sink, _capture) = common::capture_sink();
    let layer = server(sink);
    let service = layer.layer(common::responding(StatusCode::OK));

    assert!(format!("{layer:?}").contains("TelemetryLayer"));
    assert!(format!("{service:?}").contains("Telemetry<"));
}
