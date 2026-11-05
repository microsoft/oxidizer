// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Controlled fake-transport errors, buffering, and metrics.

#![cfg(feature = "test-util")]

use fetch::fake::FakeHandler;
use fetch::handlers::Buffering;
use fetch::{HttpBodyBuilder, HttpClient, HttpError, HttpResponseBuilder};
use futures::executor::block_on;
use layered::{Layer as _, Service as _};
use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};
use tick::Clock;

testing_aids::init_tracing!();

#[test]
fn gamma_survivor_buffering_preserves_stream_error() {
    let handler = Buffering::layer().layer(FakeHandler::from_fn(|_| {
        HttpResponseBuilder::new_fake()
            .stream(futures::stream::iter([Err::<bytesbuf::BytesView, _>(HttpError::validation(
                "buffer-marker",
            ))]))
            .build()
    }));
    let request = fetch::HttpRequestBuilder::new_fake().uri("https://example.test").build().unwrap();
    assert!(
        block_on(handler.execute(request))
            .unwrap_err()
            .to_string()
            .contains("buffer-marker")
    );
}

#[test]
fn gamma_survivor_relative_request_fails_before_fake_transport() {
    let client = HttpClient::builder_fake(http::StatusCode::OK, &Clock::new_frozen()).build();
    let request = http::Request::get("/relative").body(HttpBodyBuilder::new_fake().empty()).unwrap();
    block_on(client.execute(request)).unwrap_err();
}

#[test]
fn gamma_survivor_request_metric_description_and_unit_are_exact() {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
    let client = HttpClient::builder_fake(http::StatusCode::OK, &Clock::new_frozen())
        .name("gamma-client")
        .meter_provider(provider.clone())
        .build();
    block_on(client.get("https://example.test").fetch()).unwrap();
    provider.force_flush().unwrap();
    let metrics = exporter.get_finished_metrics().unwrap();
    let scopes = metrics.iter().flat_map(ResourceMetrics::scope_metrics).collect::<Vec<_>>();
    let scope = scopes
        .iter()
        .find(|scope| scope.metrics().any(|metric| metric.name() == "http.client.request.duration"))
        .unwrap();
    let attributes = scope
        .scope()
        .attributes()
        .map(|kv| (kv.key.as_str(), kv.value.as_str().into_owned()))
        .collect::<Vec<_>>();
    assert_eq!(
        attributes,
        [
            ("fetch.runtime", "fake".to_owned()),
            ("fetch.transport", "fake".to_owned()),
            ("http.client.name", "gamma-client".to_owned()),
        ]
    );
    let metric = scope
        .metrics()
        .find(|metric| metric.name() == "http.client.request.duration")
        .unwrap();
    assert_eq!(metric.description(), "Duration of HTTP client requests.");
    assert_eq!(metric.unit(), "s");
}

#[test]
fn gamma_survivor_total_metric_description_and_unit_are_exact() {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
    let client = HttpClient::builder_fake(http::StatusCode::OK, &Clock::new_frozen())
        .meter_provider(provider.clone())
        .standard_pipeline(|pipeline, _| pipeline)
        .build();
    block_on(client.get("https://example.test").fetch()).unwrap();
    provider.force_flush().unwrap();
    let resources = exporter.get_finished_metrics().unwrap();
    let metric = resources
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == "http.client.request.total_duration")
        .unwrap();
    assert_eq!(
        metric.description(),
        "Total duration of HTTP client requests, including all retries and hedged attempts."
    );
    assert_eq!(metric.unit(), "s");
}

#[test]
fn gamma_survivor_logging_uses_supplied_clock_and_redaction_engine() {
    use data_privacy::RedactionEngine;
    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use fetch::handlers::Logging;
    use testing_aids::tracing_logs::Capture;

    let capture = Capture::new();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    let engine = RedactionEngine::builder()
        .add_class_redactor(
            templated_uri::Uri::DATA_CLASS,
            SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
        )
        .build();
    let control = tick::ClockControl::new();
    let advanced = control.clone();
    let handler = Logging::layer()
        .clock(tick::SimpleClock::from(&control))
        .redaction_engine(&engine)
        .layer(FakeHandler::from_fn(move |_| {
            advanced.advance(std::time::Duration::from_millis(125));
            HttpResponseBuilder::new_fake().status(http::StatusCode::ACCEPTED).build()
        }));
    let request = fetch::HttpRequestBuilder::new_fake()
        .uri("https://example.test/path?marker=public")
        .build()
        .unwrap();
    block_on(handler.execute(request)).unwrap();
    capture.assert_contains("url.path.redacted=\"/path?marker=public\"");
    capture.assert_contains("http.response.status_code=202");
    capture.assert_contains("http.client.request.duration=0.125");
}

#[test]
fn gamma_survivor_metrics_clock_and_abandonment_callback_are_exact() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use fetch::handlers::Metrics;

    let control = tick::ClockControl::new();
    let advanced = control.clone();
    let samples = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&samples);
    let handler = Metrics::layer()
        .clock(&control)
        .on_record(move |duration, result, _| recorded.lock().unwrap().push((duration, result.is_ok())))
        .layer(FakeHandler::from_fn(move |_| {
            advanced.advance(Duration::from_millis(125));
            HttpResponseBuilder::new_fake().build()
        }));
    let request = fetch::HttpRequestBuilder::new_fake().uri("https://example.test").build().unwrap();
    block_on(handler.execute(request)).unwrap();
    assert_eq!(*samples.lock().unwrap(), [(Duration::from_millis(125), true)]);

    let errors = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&errors);
    let handler = Metrics::layer()
        .clock(&control)
        .on_record(move |_, result, _| recorded.lock().unwrap().push(result.as_ref().unwrap_err().to_string()))
        .layer(FakeHandler::never_completes());
    let request = fetch::HttpRequestBuilder::new_fake().uri("https://example.test").build().unwrap();
    let mut future = Box::pin(handler.execute(request));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    drop(future);
    let errors = errors.lock().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("the future was dropped"), "{}", errors[0]);
}

#[test]
fn gamma_survivor_two_pool_custom_factory_receives_tls_options() {
    use std::sync::{Arc, Mutex};

    use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
    use fetch::options::{ConnectionPoolOptions, PoolSelection};

    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&calls);
    let tls = fetch_tls::TlsOptions::builder()
        .client_identity(fetch_tls::ClientIdentity::from_der([b"certificate-marker"], b"key-marker"))
        .build();
    let expected_tls = format!("{tls:?}");
    let deps = CustomDeps {
        clock: Clock::new_frozen(),
        memory: bytesbuf::mem::OpaqueMemory::new(bytesbuf::mem::GlobalPool::new()),
        extras: (),
    };
    let builder = create_builder(
        "runtime-marker",
        "transport-marker",
        move |context: CustomContext<()>| {
            recorded
                .lock()
                .unwrap()
                .push((context.pool_index.index(), format!("{:?}", context.tls)));
            FakeHandler::from(http::StatusCode::OK)
        },
        Isolation::Shared,
        deps,
    )
    .tls_options(tls)
    .connection_pool_options(ConnectionPoolOptions::default().multiple_pools(2, PoolSelection::round_robin()));
    assert!(format!("{builder:?}").contains("transport: fetch::custom::Transport"));
    let client = builder.build();
    block_on(client.get("https://example.test").fetch()).unwrap();
    assert_eq!(*calls.lock().unwrap(), [(0, expected_tls.clone()), (1, expected_tls)]);
}

#[test]
fn gamma_survivor_standard_pipeline_retains_zero_hedge_configuration() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use fetch::pipeline::RecoveryMode;

    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::clone(&calls);
    let handler = FakeHandler::from_fn(move |_| {
        let status = if recorded.fetch_add(1, Ordering::SeqCst) == 0 {
            http::StatusCode::SERVICE_UNAVAILABLE
        } else {
            http::StatusCode::OK
        };
        HttpResponseBuilder::new_fake().status(status).build()
    });
    let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
    let client = HttpClient::builder_fake(handler, fetch::fake::FakeDeps { clock })
        .standard_pipeline(|pipeline, _| {
            pipeline
                .total_timeout(|layer| layer.timeout(Duration::MAX))
                .attempt_timeout(|layer| layer.timeout(Duration::MAX))
                .recovery_mode(RecoveryMode::Hedging)
                .hedging(|layer| layer.max_hedged_attempts(0))
        })
        .build();
    let response = block_on(client.get("https://example.test").fetch()).unwrap();
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
