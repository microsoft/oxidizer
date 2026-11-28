// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server and client middleware. The tests cover when the middleware emits each
//! event, their fields and metric, and the effect of the layer configuration.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::cell::Cell;
use std::task::Poll;
use std::time::Duration;

use common::{
    ABANDONED, ACTIVE, CLIENT_ABANDONED, CLIENT_COMPLETE, CLIENT_DURATION, CLIENT_ERROR, COMPLETE, Capture, Captured, DURATION, Echoing,
    Probe, Step, TOTAL_ABANDONED, TOTAL_COMPLETE, TOTAL_ERROR, Tenant, client, client_duration, data, duration, poll_frame, scripted,
    server, sorted, total_duration,
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

    server(sink.clone())
        .layer(common::responding(StatusCode::OK))
        .execute(common::purge())
        .unwrap_ready()
        .unwrap();
    client(sink)
        .layer(common::responding(StatusCode::OK))
        .execute(common::purge())
        .unwrap_ready()
        .unwrap();

    let events = capture.events();
    let fields: Vec<_> = events
        .iter()
        .map(|event| (event.name, sorted(&event.logs), sorted(&event.dimensions)))
        .collect();
    assert_eq!(
        fields,
        vec![
            (
                COMPLETE,
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
            ),
            (
                CLIENT_COMPLETE,
                sorted(&[
                    ("http.request.method", Value::from("_OTHER")),
                    ("http.request.method_original", Value::from("PURGE")),
                    ("url.scheme", Value::from("_OTHER")),
                    ("network.protocol.name", Value::from("http")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("url.template", Value::from("unknown")),
                    ("url.path.redacted", Value::from("/books/1")),
                    ("http.response.status_code", Value::I64(200)),
                    (CLIENT_DURATION, Value::F64(0.0)),
                ]),
                sorted(&[
                    ("http.request.method", Value::from("_OTHER")),
                    ("url.scheme", Value::from("_OTHER")),
                    ("network.protocol.name", Value::from("http")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("url.template", Value::from("unknown")),
                    ("http.response.status_code", Value::I64(200)),
                ]),
            ),
        ]
    );
}

#[test]
fn protocol_version_uses_open_telemetry_values() {
    let (sink, capture) = common::capture_sink();
    let versions = [
        Version::HTTP_09,
        Version::HTTP_10,
        Version::HTTP_11,
        Version::HTTP_2,
        Version::HTTP_3,
    ];
    // The server records the version of the request, and the client the version of the response.
    // `Echoing` responds with the version of the request.
    let services = [
        server(sink.clone()).layer(Echoing(StatusCode::OK)),
        client(sink).layer(Echoing(StatusCode::OK)),
    ];

    for service in &services {
        for version in versions {
            service.execute(common::request(Method::GET, version)).unwrap_ready().unwrap();
        }
    }

    let versions: Vec<_> = capture
        .events()
        .iter()
        .map(|event| (event.name, event.log("network.protocol.version").cloned()))
        .collect();
    let expected = |name| ["_OTHER", "1.0", "1.1", "2", "3"].map(|version| (name, Some(Value::from(version))));
    assert_eq!(versions, [expected(COMPLETE), expected(CLIENT_COMPLETE)].concat());
}

#[test]
fn start_scope_enrichment_reaches_server_and_client_events() {
    let capture = Capture::new(|_| true);
    let sink = common::sink_with(&capture);
    let completing = [
        server(sink.clone()).layer(common::responding(StatusCode::OK)),
        client(sink.clone()).layer(common::responding(StatusCode::OK)),
    ];
    let abandoning = [
        server(sink.clone()).layer(common::NeverResponding),
        client(sink.clone()).layer(common::NeverResponding),
    ];
    let streaming = server(sink.clone()).layer(Echoing(StatusCode::OK));
    let streamed_request = common::get().map(|_| scripted([data("chunk"), Step::End]));

    let (complete, abandoned, streamed) = (|| {
        (
            completing.each_ref().map(|service| service.execute(common::get())),
            abandoning.each_ref().map(|service| service.execute(common::get())),
            streaming.execute(streamed_request),
        )
    })
    .enrich(&sink, Tenant { tenant: "contoso" })();
    for response in complete {
        response.unwrap_ready().unwrap();
    }
    drop(abandoned);
    // The guard moves into the response body, which ends after the scope.
    let mut body = streamed.unwrap_ready().unwrap().into_body();
    let _ = poll_frame(&mut body);
    let _ = poll_frame(&mut body);

    let enrichments: Vec<_> = capture.events().into_iter().map(|event| (event.name, event.enrichments)).collect();
    let tenant = vec![("tenant", Value::from("contoso"))];
    assert_eq!(
        enrichments,
        [
            ACTIVE,
            ACTIVE,
            ACTIVE,
            COMPLETE,
            ACTIVE,
            CLIENT_COMPLETE,
            ABANDONED,
            ACTIVE,
            CLIENT_ABANDONED,
            COMPLETE,
            ACTIVE
        ]
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
fn completion_interest_skips_error_and_abandonment_probes() {
    std::thread_local! {
        static OTHER_PROBES: Cell<usize> = const { Cell::new(0) };
    }

    let capture = Capture::new(|name| {
        if matches!(name, common::ERROR | ABANDONED) {
            OTHER_PROBES.set(OTHER_PROBES.get() + 1);
        }
        name == COMPLETE
    });
    let service = server(common::sink_with(&capture)).layer(common::responding(StatusCode::OK));
    let before = OTHER_PROBES.get();

    let response = service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(capture.names(), vec![COMPLETE]);
    assert_eq!(OTHER_PROBES.get(), before);
}

#[test]
fn interest_is_checked_at_request_start() {
    let probe = Probe::default();
    let sink = common::probe_sink(&probe);
    let services = [
        server(sink.clone()).layer(common::NeverResponding),
        client(sink).layer(common::NeverResponding),
    ];

    let uninterested = services.each_ref().map(|service| service.execute(common::get()));
    probe.set_interested(true);
    let interested = services.each_ref().map(|service| service.execute(common::get()));
    drop(uninterested);
    drop(interested);

    assert_eq!(probe.received(), vec![ACTIVE, ABANDONED, ACTIVE, CLIENT_ABANDONED]);
}

#[test]
fn client_complete_event_reports_method_status_version_and_duration() {
    let (sink, capture) = common::capture_sink();
    let clock = ClockControl::new();
    // On the client, a status from 500 to 599 is a complete request, without `error.type`.
    let service = client(sink).clock(clock.to_simple_clock()).layer(common::responding_after(
        &clock,
        Duration::from_millis(125),
        StatusCode::SERVICE_UNAVAILABLE,
    ));

    let response = service.execute(common::get_book()).unwrap_ready().unwrap();

    assert_eq!(
        (response.status(), capture.single()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Captured {
                name: CLIENT_COMPLETE,
                severity: Some(Severity::Debug),
                message: Some("HTTP request attempt completed".into()),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.name", Value::from("http")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("url.template", Value::from("/books/{book}")),
                    ("url.path.redacted", Value::from("/books/1")),
                    ("http.response.status_code", Value::I64(503)),
                    (CLIENT_DURATION, Value::F64(0.125)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("url.scheme", Value::from("https")),
                    ("network.protocol.name", Value::from("http")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("server.address", Value::from("example.com")),
                    ("server.port", Value::I64(443)),
                    ("url.template", Value::from("/books/{book}")),
                    ("http.response.status_code", Value::I64(503)),
                ],
                measurements: vec![client_duration(0.125)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn client_dropped_future_reports_abandoned_without_status() {
    let capture = Capture::new(|name| name == CLIENT_ABANDONED);
    let clock = ClockControl::new();
    let service = client(common::sink_with(&capture))
        .clock(clock.to_simple_clock())
        .layer(common::NeverResponding);

    let mut future = Box::pin(service.execute(common::get_book()));
    assert!(poll_once(&mut future).is_pending());
    clock.advance(Duration::from_millis(40));
    drop(future);

    assert_eq!(
        capture.single(),
        Captured {
            name: CLIENT_ABANDONED,
            severity: Some(Severity::Warn),
            message: Some("HTTP request attempt abandoned".into()),
            logs: vec![
                ("http.request.method", Value::from("GET")),
                ("url.scheme", Value::from("https")),
                ("server.address", Value::from("example.com")),
                ("server.port", Value::I64(443)),
                ("url.template", Value::from("/books/{book}")),
                ("url.path.redacted", Value::from("/books/1")),
                ("error.type", Value::from("abandoned")),
                (CLIENT_DURATION, Value::F64(0.04)),
            ],
            dimensions: vec![
                ("http.request.method", Value::from("GET")),
                ("url.scheme", Value::from("https")),
                ("server.address", Value::from("example.com")),
                ("server.port", Value::I64(443)),
                ("url.template", Value::from("/books/{book}")),
                ("error.type", Value::from("abandoned")),
            ],
            measurements: vec![client_duration(0.04)],
            enrichments: vec![],
        }
    );
}

#[test]
fn report_total_duration_switches_to_total_events() {
    let events_of = |enabled: bool| {
        let (sink, capture) = common::capture_sink();
        let layer = client(sink).report_total_duration(enabled);
        layer
            .layer(common::responding(StatusCode::OK))
            .execute(common::get())
            .unwrap_ready()
            .unwrap();
        layer.layer(common::FAILING).execute(common::get()).unwrap_ready().unwrap_err();
        // The future is dropped before its first poll.
        drop(layer.layer(common::NeverResponding).execute(common::get()));
        let events: Vec<_> = capture
            .events()
            .into_iter()
            .map(|event| (event.name, event.severity, event.message, event.measurements))
            .collect();
        events
    };

    assert_eq!(
        [events_of(true), events_of(false)],
        [
            vec![
                (
                    TOTAL_COMPLETE,
                    Some(Severity::Debug),
                    Some("HTTP request completed".into()),
                    vec![total_duration(0.0)]
                ),
                (
                    TOTAL_ERROR,
                    Some(Severity::Warn),
                    Some("HTTP request failed".into()),
                    vec![total_duration(0.0)]
                ),
                (
                    TOTAL_ABANDONED,
                    Some(Severity::Warn),
                    Some("HTTP request abandoned".into()),
                    vec![total_duration(0.0)]
                ),
            ],
            vec![
                (
                    CLIENT_COMPLETE,
                    Some(Severity::Debug),
                    Some("HTTP request attempt completed".into()),
                    vec![client_duration(0.0)]
                ),
                (
                    CLIENT_ERROR,
                    Some(Severity::Warn),
                    Some("HTTP request attempt failed".into()),
                    vec![client_duration(0.0)]
                ),
                (
                    CLIENT_ABANDONED,
                    Some(Severity::Warn),
                    Some("HTTP request attempt abandoned".into()),
                    vec![client_duration(0.0)]
                ),
            ],
        ]
    );
}

#[test]
fn processor_interested_only_in_total_events_receives_them() {
    let capture = Capture::new(|name| name == TOTAL_COMPLETE);
    let service = client(common::sink_with(&capture))
        .report_total_duration(true)
        .layer(common::responding(StatusCode::OK));

    service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!(capture.names(), vec![TOTAL_COMPLETE]);
}

#[test]
fn client_finishes_at_handler_return_without_reading_body() {
    let (sink, capture) = common::capture_sink();
    let service = client(sink).layer(Echoing(StatusCode::OK));

    let mut body = service
        .execute(common::get().map(|_| scripted([data("chunk"), Step::End])))
        .unwrap_ready()
        .unwrap()
        .into_body();
    let names = capture.names();
    let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
        panic!("the first poll must return the frame that the handler sent");
    };

    assert_eq!(
        (names, frame.into_data().unwrap().to_vec()),
        (vec![CLIENT_COMPLETE], b"chunk".to_vec())
    );
}

#[test]
fn layer_and_middleware_debug_output_names_their_types() {
    let (sink, _capture) = common::capture_sink();
    let layer = server(sink);
    let service = layer.layer(common::responding(StatusCode::OK));

    assert!(format!("{layer:?}").contains("TelemetryLayer"));
    assert!(format!("{service:?}").contains("Telemetry<"));
    assert!(format!("{service:?}").contains("inner: Responding"));
}
