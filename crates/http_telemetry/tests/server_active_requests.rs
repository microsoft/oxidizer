// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The deltas of `http.server.active_requests`: when the middleware emits them, their fields, and
//! how each pairs with the complete, error or abandoned event of its request.

#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::future::{poll_fn, ready};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use common::{ABANDONED, ACTIVE, COMPLETE, Capture, Captured, ERROR, Measurement, Tenant, server};
use future_assert::{FutureTestExt, poll_once};
use http::{Method, StatusCode};
use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse};
use http_telemetry::TelemetryLayer;
use layered::{Execute, Layer, Service};
use observed::enrichment::EnrichFnExt;
use observed::metadata::InstrumentKind;
use observed::{Severity, Value};

/// Runs `request` with a server layer over a sink that captures every event. Returns the name of
/// each event with its delta, if it is a delta of `http.server.active_requests`.
fn events_of(request: impl FnOnce(TelemetryLayer)) -> Vec<(&'static str, Option<Value>)> {
    let capture = Capture::new(|_| true);
    request(server(common::sink_with(&capture)));
    capture
        .events()
        .into_iter()
        .map(|event| (event.name, event.log(ACTIVE).cloned()))
        .collect()
}

#[test]
fn active_requests_rises_before_handler_and_falls_after_event() {
    let capture = Capture::new(|_| true);
    let seen_by_handler = Arc::new(Mutex::new(Vec::new()));
    let handler = {
        let (capture, seen_by_handler) = (capture.clone(), Arc::clone(&seen_by_handler));
        Execute::new(move |_: HttpRequest| {
            *seen_by_handler.lock().unwrap() = capture.names();
            ready(Ok::<_, HttpError>(HttpResponse::new(HttpBodyBuilder::new_fake().empty())))
        })
    };
    let service = server(common::sink_with(&capture)).layer(handler);

    service.execute(common::get()).unwrap_ready().unwrap();

    assert_eq!(
        (seen_by_handler.lock().unwrap().clone(), capture.names()),
        (vec![ACTIVE], vec![ACTIVE, COMPLETE, ACTIVE])
    );
}

#[test]
fn active_requests_logs_and_meters_each_delta() {
    // The processor wants the counter alone, so the middleware runs for the counter alone.
    let capture = Capture::new(|name| name == ACTIVE);
    let service = server(common::sink_with(&capture)).layer(common::responding(StatusCode::OK));
    let mut request = common::get_to("https://example.com/books/1");
    *request.method_mut() = Method::from_bytes(b"PURGE").unwrap();

    service.execute(request).unwrap_ready().unwrap();

    let delta = |value: i64| Captured {
        name: ACTIVE,
        severity: Some(Severity::Debug),
        message: Some("Active HTTP server requests changed".into()),
        timestamp: common::sink_time(),
        logs: vec![
            (ACTIVE, Value::I64(value)),
            ("http.request.method", Value::from("_OTHER")),
            ("url.scheme", Value::from("https")),
        ],
        dimensions: vec![("http.request.method", Value::from("_OTHER")), ("url.scheme", Value::from("https"))],
        measurements: vec![Measurement {
            instrument: ACTIVE,
            kind: InstrumentKind::UpDownCounter,
            description: "Number of active HTTP server requests.",
            unit: "{request}",
            value: Value::I64(value),
        }],
        enrichments: vec![],
    };
    assert_eq!(capture.events(), vec![delta(1), delta(-1)]);
}

#[test]
fn every_event_pairs_active_requests() {
    let handler_panic =
        || Execute::new(|_: HttpRequest| poll_fn(|_| -> Poll<http_extensions::Result<HttpResponse>> { panic!("the handler panicked") }));

    let rows = [
        events_of(|layer| {
            drop(
                layer
                    .layer(common::responding(StatusCode::OK))
                    .execute(common::get())
                    .unwrap_ready(),
            );
        }),
        events_of(|layer| {
            drop(
                layer
                    .layer(common::responding(StatusCode::NOT_FOUND))
                    .execute(common::get())
                    .unwrap_ready(),
            );
        }),
        events_of(|layer| {
            drop(
                layer
                    .layer(common::responding(StatusCode::SERVICE_UNAVAILABLE))
                    .execute(common::get())
                    .unwrap_ready(),
            );
        }),
        events_of(|layer| drop(layer.layer(common::FAILING).execute(common::get()).unwrap_ready())),
        events_of(|layer| {
            let service = layer.layer(handler_panic());
            drop(catch_unwind(AssertUnwindSafe(|| service.execute(common::get()).unwrap_ready())));
        }),
        events_of(|layer| {
            let service = layer.layer(common::NeverResponding);
            let mut future = Box::pin(service.execute(common::get()));
            assert!(poll_once(&mut future).is_pending());
        }),
        events_of(|layer| drop(layer.layer(common::NeverResponding).execute(common::get()))),
    ];

    let paired = |name| vec![(ACTIVE, Some(Value::I64(1))), (name, None), (ACTIVE, Some(Value::I64(-1)))];
    assert_eq!(
        rows,
        [
            paired(COMPLETE),
            paired(COMPLETE),
            paired(COMPLETE),
            paired(ERROR),
            paired(ERROR),
            paired(ABANDONED),
            paired(ABANDONED)
        ]
    );
}

#[test]
fn active_requests_decrement_carries_start_enrichment() {
    let capture = Capture::new(|name| name == ACTIVE);
    let sink = common::sink_with(&capture);
    let service = server(sink.clone()).layer(common::responding(StatusCode::OK));

    let future = (|| service.execute(common::get())).enrich(&sink, Tenant { tenant: "contoso" })();
    future.unwrap_ready().unwrap();

    let enrichments: Vec<_> = capture.events().into_iter().map(|event| event.enrichments).collect();
    assert_eq!(enrichments, vec![vec![("tenant", Value::from("contoso"))]; 2]);
}

#[test]
fn request_dropped_during_unwind_emits_no_decrement() {
    let capture = Capture::new(|_| true);
    let service = server(common::sink_with(&capture)).layer(common::NeverResponding);
    let mut future = Box::pin(service.execute(common::get()));
    assert!(poll_once(&mut future).is_pending());

    let unwinding = catch_unwind(AssertUnwindSafe(move || {
        let _future = future;
        panic!("the caller panicked");
    }));

    assert_eq!((unwinding.is_err(), capture.names()), (true, vec![ACTIVE]));
}
