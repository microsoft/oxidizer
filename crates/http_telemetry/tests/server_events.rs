// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The events of the server middleware. The tests cover when the middleware emits each event,
//! their fields and metric, and the effect of the layer configuration.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::collections::HashSet;
use std::time::Duration;

use common::{Captured, Measurement, Probe};
use future_assert::{FutureTestExt, poll_once};
use http::{Method, StatusCode, Version};
use http_telemetry::{Telemetry, TelemetryLayer};
use layered::{Layer, Service};
use observed::enrichment::EnrichFnExt;
use observed::metadata::{EventDescription, InstrumentKind};
use observed::{Enrichment, Severity, Sink, Value};
use tick::{ClockControl, SimpleClock};

const COMPLETE: &str = "http.server.request.complete";
const ABANDONED: &str = "http.server.request.abandoned";
const DURATION: &str = "http.server.request.duration";

/// A server layer over `sink` with a frozen clock. If a test does not set its own clock, each
/// duration is `0`.
fn server(sink: Sink) -> TelemetryLayer {
    Telemetry::server(sink).clock(SimpleClock::new_frozen())
}

fn duration(value: u64) -> Measurement {
    Measurement {
        instrument: DURATION,
        kind: InstrumentKind::Histogram,
        description: "Duration of HTTP server requests.",
        unit: "ms",
        value: Value::U64(value),
    }
}

#[derive(Enrichment)]
struct Tenant {
    #[dimension(metric = "tenant")]
    #[unredacted]
    tenant: &'static str,
}

#[test]
fn server_complete_event_reports_method_status_version_and_duration() {
    let (sink, capture) = common::capture_sink();
    let clock = ClockControl::new();
    let service =
        server(sink)
            .clock(clock.to_simple_clock())
            .layer(common::responding_after(&clock, Duration::from_millis(125), StatusCode::OK));

    let response = service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!(
        (response.status(), capture.single()),
        (
            StatusCode::OK,
            Captured {
                name: COMPLETE,
                severity: Some(Severity::Info),
                message: Some("HTTP server request completed".into()),
                timestamp: common::sink_time(),
                logs: vec![
                    ("http.request.method", Value::from("GET")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("http.response.status_code", Value::I64(200)),
                    (DURATION, Value::U64(125)),
                ],
                dimensions: vec![
                    ("http.request.method", Value::from("GET")),
                    ("network.protocol.version", Value::from("1.1")),
                    ("http.response.status_code", Value::I64(200)),
                ],
                measurements: vec![duration(125)],
                enrichments: vec![],
            }
        )
    );
}

#[test]
fn server_dropped_future_reports_abandoned_without_status() {
    let (sink, capture) = common::capture_sink();
    let clock = ClockControl::new();
    let service = server(sink).clock(clock.to_simple_clock()).layer(common::NeverResponding);

    let mut future = Box::pin(service.execute(common::get()));
    assert!(poll_once(&mut future).is_pending());
    clock.advance(Duration::from_millis(40));
    drop(future);

    assert_eq!(
        capture.single(),
        Captured {
            name: ABANDONED,
            severity: Some(Severity::Warn),
            message: Some("HTTP server request abandoned".into()),
            timestamp: common::sink_time(),
            logs: vec![
                ("http.request.method", Value::from("GET")),
                ("network.protocol.version", Value::from("1.1")),
                ("error.type", Value::from("abandoned")),
                (DURATION, Value::U64(40)),
            ],
            dimensions: vec![
                ("http.request.method", Value::from("GET")),
                ("network.protocol.version", Value::from("1.1")),
                ("error.type", Value::from("abandoned")),
            ],
            measurements: vec![duration(40)],
            enrichments: vec![],
        }
    );
}

#[test]
fn unpolled_request_future_reports_abandoned() {
    let (sink, capture) = common::capture_sink();
    let service = server(sink).layer(common::NeverResponding);

    drop(service.execute(common::get()));

    assert_eq!(capture.names(), vec![ABANDONED]);
}

#[test]
fn each_event_has_own_type_id() {
    let probe = Probe::default();
    let service = server(common::probe_sink(&probe)).layer(common::responding(StatusCode::OK));

    service.execute(common::get()).unwrap_ready().unwrap();

    let asked = probe.asked();
    let mut names: Vec<_> = asked.iter().map(EventDescription::name).collect();
    names.sort_unstable();
    let type_ids: HashSet<_> = asked.iter().map(|description| description.type_id().unwrap()).collect();
    assert_eq!((names, type_ids.len()), (vec![ABANDONED, COMPLETE], 2));
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
        (event.logs, event.dimensions),
        (
            vec![
                ("http.request.method", Value::from("_OTHER")),
                ("http.request.method_original", Value::from("PURGE")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(200)),
                (DURATION, Value::U64(0)),
            ],
            vec![
                ("http.request.method", Value::from("_OTHER")),
                ("network.protocol.version", Value::from("1.1")),
                ("http.response.status_code", Value::I64(200)),
            ],
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
    let (sink, capture) = common::capture_sink();
    let completing = server(sink.clone()).layer(common::responding(StatusCode::OK));
    let abandoning = server(sink.clone()).layer(common::NeverResponding);

    let (complete, abandoned) =
        (|| (completing.execute(common::get()), abandoning.execute(common::get()))).enrich(&sink, Tenant { tenant: "contoso" })();
    complete.unwrap_ready().unwrap();
    drop(abandoned);

    let enrichments: Vec<_> = capture.events().into_iter().map(|event| (event.name, event.enrichments)).collect();
    assert_eq!(
        enrichments,
        vec![
            (COMPLETE, vec![("tenant", Value::from("contoso"))]),
            (ABANDONED, vec![("tenant", Value::from("contoso"))]),
        ]
    );
}

#[test]
fn uninterested_sink_forwards_request_untouched() {
    let probe = Probe::default();
    let service = server(common::probe_sink(&probe)).layer(common::responding(StatusCode::IM_A_TEAPOT));

    let response = service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!((response.status(), probe.received()), (StatusCode::IM_A_TEAPOT, vec![]));
}

#[test]
fn interest_is_checked_for_each_request() {
    let probe = Probe::default();
    let service = server(common::probe_sink(&probe)).layer(common::responding(StatusCode::OK));

    service.execute(common::get()).unwrap_ready().unwrap();
    probe.set_interested(true);
    service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!(probe.received(), vec![COMPLETE]);
}
