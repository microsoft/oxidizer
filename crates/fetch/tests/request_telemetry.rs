// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests of the request telemetry that `fetch` emits through the sink of its deps.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::fake::{FakeDeps, FakeHandler};
use fetch::pipeline::RecoveryMode;
use fetch::{HttpClient, HttpClientBuilder, HttpResponseBuilder};
use futures::executor::block_on;
use http::StatusCode;
use layered::Stack;
use observed::{Severity, Sink, SinkId};
use observed_testing::{CapturedEvent, MockProcessor, test_emitter};
use opentelemetry::metrics::NoopMeterProvider;
use tick::{Clock, ClockControl, SimpleClock};

const URI: &str = "https://example.com/books";

const ATTEMPT_COMPLETE: &str = "http.client.request.complete";
const ATTEMPT_ABANDONED: &str = "http.client.request.abandoned";
const TOTAL_COMPLETE: &str = "http.client.request.total.complete";
const TOTAL_ERROR: &str = "http.client.request.total.error";

#[test]
fn standard_pipeline_emits_attempt_and_total_events() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink)).build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        instruments(&processor),
        [
            (ATTEMPT_COMPLETE.to_owned(), "http.client.request.duration"),
            (TOTAL_COMPLETE.to_owned(), "http.client.request.total_duration"),
        ]
    );
}

#[test]
fn retry_emits_each_attempt_and_one_total() {
    let (sink, processor) = capture();
    let calls = AtomicUsize::new(0);
    let handler = FakeHandler::from_fn(move |_| {
        let status = if calls.fetch_add(1, Ordering::Relaxed) < 3 {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        };
        HttpResponseBuilder::new_fake().status(status).build()
    });
    let client = HttpClient::builder_fake(handler, FakeDeps::with_clock(&auto_advanced_clock(), sink))
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|timeout| timeout.timeout(Duration::MAX))
                .attempt_timeout(|timeout| timeout.timeout(Duration::MAX))
                .retry(|retry| retry.max_retry_attempts(3).base_delay(Duration::from_millis(1)))
        })
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        outcomes(&processor),
        [
            outcome(ATTEMPT_COMPLETE, Some("500"), None),
            outcome(ATTEMPT_COMPLETE, Some("500"), None),
            outcome(ATTEMPT_COMPLETE, Some("500"), None),
            outcome(ATTEMPT_COMPLETE, Some("200"), None),
            outcome(TOTAL_COMPLETE, Some("200"), None),
        ]
    );
}

#[test]
fn hedge_loser_reports_abandoned_and_total_reports_once() {
    let (sink, processor) = capture();
    let calls = AtomicUsize::new(0);
    let handler = FakeHandler::from_async_fn(move |_| {
        let first = calls.fetch_add(1, Ordering::Relaxed) == 0;
        async move {
            if first {
                pending::<()>().await;
            }
            HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
        }
    });
    let client = HttpClient::builder_fake(handler, FakeDeps::with_clock(&auto_advanced_clock(), sink))
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|timeout| timeout.timeout(Duration::MAX))
                .attempt_timeout(|timeout| timeout.timeout(Duration::MAX))
                .recovery_mode(RecoveryMode::Hedging)
                .hedging(|hedging| hedging.max_hedged_attempts(1).hedging_delay(Duration::from_millis(100)))
        })
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    let events = processor
        .events()
        .iter()
        .map(|event| (event.name().to_owned(), event.severity(), field(event, "error.type")))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            (ATTEMPT_COMPLETE.to_owned(), Some(Severity::Debug), None),
            (ATTEMPT_ABANDONED.to_owned(), Some(Severity::Warn), Some("abandoned".to_owned())),
            (TOTAL_COMPLETE.to_owned(), Some(Severity::Debug), None),
        ]
    );
}

#[test]
fn attempt_timeout_reports_abandoned_attempt() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps::with_clock(&auto_advanced_clock(), sink))
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|timeout| timeout.timeout(Duration::MAX))
                .attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(1)))
        })
        .build();

    block_on(client.get(URI).fetch()).unwrap_err();

    // The standard retry retries each timeout three times.
    let abandoned = outcome(ATTEMPT_ABANDONED, None, Some("abandoned"));
    assert_eq!(
        outcomes(&processor),
        [
            abandoned.clone(),
            abandoned.clone(),
            abandoned.clone(),
            abandoned,
            outcome(TOTAL_ERROR, None, Some("response_timeout")),
        ]
    );
}

#[test]
fn total_timeout_reports_one_total_error() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps::with_clock(&auto_advanced_clock(), sink))
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|timeout| timeout.timeout(Duration::from_secs(1)))
                .attempt_timeout(|timeout| timeout.timeout(Duration::MAX))
        })
        .build();

    block_on(client.get(URI).fetch()).unwrap_err();

    assert_eq!(
        outcomes(&processor),
        [
            outcome(ATTEMPT_ABANDONED, None, Some("abandoned")),
            outcome(TOTAL_ERROR, None, Some("response_timeout")),
        ]
    );
}

#[test]
fn breaker_rejection_invents_no_attempt() {
    let (sink, processor) = capture();
    let handler = FakeHandler::from_fn(|_| HttpResponseBuilder::new_fake().status(StatusCode::INTERNAL_SERVER_ERROR).build());
    let client = HttpClient::builder_fake(handler, FakeDeps::with_clock(&auto_advanced_clock(), sink))
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|timeout| timeout.timeout(Duration::MAX))
                .attempt_timeout(|timeout| timeout.timeout(Duration::MAX))
                .retry(|retry| retry.max_retry_attempts(3).base_delay(Duration::from_millis(1)))
                .breaker(|breaker| {
                    breaker
                        .min_throughput(5)
                        .failure_threshold(0.5)
                        .break_duration(Duration::from_mins(1))
                })
        })
        .build();
    // The failures of these requests open the breaker.
    for _ in 0..10 {
        drop(block_on(client.get(URI).fetch()));
    }
    let before = processor.len();

    block_on(client.get(URI).fetch()).unwrap_err();

    assert_eq!(outcomes(&processor)[before..], [outcome(TOTAL_ERROR, None, Some("unavailable"))]);
}

#[test]
fn request_events_have_client_attributes() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink))
        .name("books_client")
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(client_attributes(&processor), vec![attributes("fake", "fake", "books_client"); 2]);
}

#[test]
fn custom_transport_names_reach_request_events() {
    let (sink, processor) = capture();
    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        memory: OpaqueMemory::new(GlobalPool::new()),
        sink,
        extras: (),
    };
    let client = create_builder(
        "my-runtime",
        "my-custom",
        |_: CustomContext| FakeHandler::from(StatusCode::OK),
        Isolation::Shared,
        deps,
    )
    .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        client_attributes(&processor),
        vec![attributes("my-runtime", "my-custom", "http_client"); 2]
    );
}

#[test]
fn client_name_set_before_meter_provider_reaches_request_events() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink))
        .name("named_before_provider")
        .meter_provider(NoopMeterProvider::new())
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        client_attributes(&processor),
        vec![attributes("fake", "fake", "named_before_provider"); 2]
    );
}

#[test]
fn client_name_set_after_meter_provider_reaches_request_events() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink))
        .meter_provider(NoopMeterProvider::new())
        .name("named_after_provider")
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        client_attributes(&processor),
        vec![attributes("fake", "fake", "named_after_provider"); 2]
    );
}

#[test]
fn custom_pipeline_with_client_telemetry_emits_client_attributes() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink))
        .name("custom_client")
        .custom_pipeline(|dispatch, ctx| (ctx.client_telemetry(), dispatch).into_service())
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(client_attributes(&processor), [attributes("fake", "fake", "custom_client")]);
}

#[test]
fn custom_pipeline_without_client_telemetry_emits_no_request_events() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink))
        .custom_pipeline(|dispatch, _| dispatch)
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert!(processor.is_empty());
}

#[test]
fn minimal_pipeline_emits_no_request_events() {
    let (sink, processor) = capture();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink))
        .minimal_pipeline()
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert!(processor.is_empty());
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover SDK metrics initialization, which reads the OS clock")]
fn request_duration_is_not_recorded_on_meter() {
    use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        memory: OpaqueMemory::new(GlobalPool::new()),
        sink: Sink::noop(),
        extras: (),
    };
    // The transport records a counter on the `fetch` meter, so the meter has a data point to export.
    let client = create_builder(
        "test-runtime",
        "test",
        |cx: CustomContext| {
            cx.meter.u64_counter("custom.transport.requests").build().add(1, &[]);
            FakeHandler::from(StatusCode::OK)
        },
        Isolation::Shared,
        deps,
    )
    .meter_provider(provider.clone())
    .build();

    block_on(client.get(URI).fetch()).unwrap();
    provider.force_flush().unwrap();

    let names = exporter
        .get_finished_metrics()
        .unwrap()
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .map(|metric| metric.name().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["custom.transport.requests"]);
}

#[test]
fn attempt_telemetry_configures_attempt_events_only() {
    let (sink, processor) = capture();
    let control = ClockControl::new();
    let total_clock = control.to_simple_clock();
    let client = advancing_client(&control, sink, Duration::from_millis(10))
        .standard_pipeline(move |pipeline, _| {
            let total_clock = total_clock.clone();
            pipeline
                .attempt_telemetry(|telemetry| telemetry.clock(SimpleClock::new_frozen()))
                .total_telemetry(|telemetry| telemetry.clock(total_clock))
        })
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        durations(&processor),
        [(ATTEMPT_COMPLETE.to_owned(), 0.0), (TOTAL_COMPLETE.to_owned(), 0.01)]
    );
}

#[test]
fn total_telemetry_configures_total_events_only() {
    let (sink, processor) = capture();
    let control = ClockControl::new();
    let attempt_clock = control.to_simple_clock();
    let client = advancing_client(&control, sink, Duration::from_millis(10))
        .standard_pipeline(move |pipeline, _| {
            let attempt_clock = attempt_clock.clone();
            pipeline
                .attempt_telemetry(|telemetry| telemetry.clock(attempt_clock))
                .total_telemetry(|telemetry| telemetry.clock(SimpleClock::new_frozen()))
        })
        .build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        durations(&processor),
        [(ATTEMPT_COMPLETE.to_owned(), 0.01), (TOTAL_COMPLETE.to_owned(), 0.0)]
    );
}

#[test]
fn request_events_use_client_clock() {
    let (sink, processor) = capture();
    let control = ClockControl::new();
    let client = advancing_client(&control, sink, Duration::from_millis(125)).build();

    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(
        durations(&processor),
        [(ATTEMPT_COMPLETE.to_owned(), 0.125), (TOTAL_COMPLETE.to_owned(), 0.125)]
    );
}

fn capture() -> (Sink, MockProcessor) {
    test_emitter(SinkId::new("request_telemetry"))
}

/// A clock that fires each timer at once, so retry delays and timeouts pass without a wait.
fn auto_advanced_clock() -> Clock {
    ClockControl::new().auto_advance_timers(true).to_clock()
}

/// A fake client on the clock of `control`, whose handler moves that clock forward by `time` and
/// then responds with 200.
fn advancing_client(control: &ClockControl, sink: Sink, time: Duration) -> HttpClientBuilder {
    let advanced = control.clone();
    let handler = FakeHandler::from_fn(move |_| {
        advanced.advance(time);
        HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
    });
    HttpClient::builder_fake(handler, FakeDeps::with_clock(&control.to_clock(), sink))
}

/// Returns the text of the value of the field `key` of `event`.
fn field(event: &CapturedEvent, key: &str) -> Option<String> {
    event
        .dimensions()
        .into_iter()
        .find_map(|(field_key, value)| (field_key == key).then(|| value.to_string()))
}

/// The name of an event, with its `http.response.status_code` and its `error.type`.
type Outcome = (String, Option<String>, Option<String>);

fn outcome(name: &str, status: Option<&str>, error_type: Option<&str>) -> Outcome {
    (name.to_owned(), status.map(str::to_owned), error_type.map(str::to_owned))
}

fn outcomes(processor: &MockProcessor) -> Vec<Outcome> {
    processor
        .events()
        .iter()
        .map(|event| {
            (
                event.name().to_owned(),
                field(event, "http.response.status_code"),
                field(event, "error.type"),
            )
        })
        .collect()
}

/// The `fetch.runtime`, the `fetch.transport` and the `http.client.name` of an event.
type Attributes = [Option<String>; 3];

fn attributes(runtime: &str, transport: &str, client_name: &str) -> Attributes {
    [Some(runtime.to_owned()), Some(transport.to_owned()), Some(client_name.to_owned())]
}

fn client_attributes(processor: &MockProcessor) -> Vec<Attributes> {
    processor
        .events()
        .iter()
        .map(|event| {
            [
                field(event, "fetch.runtime"),
                field(event, "fetch.transport"),
                field(event, "http.client.name"),
            ]
        })
        .collect()
}

/// Returns the name of each event with the instrument that it records.
fn instruments(processor: &MockProcessor) -> Vec<(String, &'static str)> {
    processor
        .events()
        .iter()
        .flat_map(|event| {
            event
                .field_metrics()
                .iter()
                .map(|metric| (event.name().to_owned(), metric.metric().instrument_name()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Returns the name of each event with the duration that it records, in seconds.
fn durations(processor: &MockProcessor) -> Vec<(String, f64)> {
    processor
        .events()
        .iter()
        .flat_map(|event| {
            event
                .field_metrics()
                .iter()
                .map(|metric| (event.name().to_owned(), metric.value()))
                .collect::<Vec<_>>()
        })
        .collect()
}

testing_aids::init_tracing!();
