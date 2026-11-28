// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Controlled fake-transport errors, buffering, and pipeline configuration.

#![cfg(feature = "test-util")]

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::handlers::Buffering;
use fetch::{HttpBodyBuilder, HttpClient, HttpError, HttpResponseBuilder};
use futures::executor::block_on;
use layered::{Layer as _, Service as _};
use observed::Sink;
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
    let client = HttpClient::builder_fake(http::StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), Sink::noop())).build();
    let request = http::Request::get("/relative").body(HttpBodyBuilder::new_fake().empty()).unwrap();
    block_on(client.execute(request)).unwrap_err();
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
        sink: Sink::noop(),
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
    assert!(
        format!("{builder:?}")
            .contains("transport: fetch::custom::Transport { runtime: \"runtime-marker\", transport: \"transport-marker\", .. }")
    );
    let client = builder.build();
    block_on(client.get("https://example.test").fetch()).unwrap();
    assert_eq!(*calls.lock().unwrap(), [(0, expected_tls.clone()), (1, expected_tls)]);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "native tests cover standard pipeline SDK metrics initialization and hedge configuration"
)]
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
    let client = HttpClient::builder_fake(handler, FakeDeps::with_clock(&clock, Sink::noop()))
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
