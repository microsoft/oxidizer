// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::missing_panics_doc,
    clippy::wildcard_imports,
    reason = "improves readability in benchmarks"
)]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![expect(missing_docs, reason = "Benchmark code")]

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use fetch::HttpClient;
use fetch::handlers::{Logging, Metrics};
use fetch::resilience::retry::HttpRetryLayerExt;
use fetch::resilience::timeout::HttpTimeoutLayerExt;
use futures::executor::block_on;
use http::StatusCode;
use layered::Stack;
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const URI_STRING: &str = "https://example.com/some/path?query=value";

fn get_uri() -> &'static str {
    URI_STRING
}

fn entry(c: &mut Criterion) {
    let session = Session::new();
    let mut group = c.benchmark_group("http_client_pipelines");

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen()).build();
    let standard_allocs = session.operation("standard_pipeline");
    group.bench_function("standard_pipeline", |b| {
        b.iter(|| {
            let _measure = standard_allocs.measure_thread();
            _ = block_on(client.get(get_uri()).fetch()).unwrap();
        });
    });

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .minimal_pipeline()
        .build();
    let minimal_allocs = session.operation("minimal_pipeline");
    group.bench_function("minimal_pipeline", |b| {
        b.iter(|| {
            let _measure = minimal_allocs.measure_thread();
            _ = block_on(client.get(get_uri()).fetch()).unwrap();
        });
    });

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .custom_pipeline(|dispatch, _context| dispatch)
        .build();
    let custom_minimal_allocs = session.operation("custom_minimal_pipeline");
    group.bench_function("custom_minimal_pipeline", |b| {
        b.iter(|| {
            let _measure = custom_minimal_allocs.measure_thread();
            _ = block_on(client.get(get_uri()).fetch()).unwrap();
        });
    });

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .custom_pipeline(|dispatch, context| {
            let stack = (
                Timeout::layer("total_timeout", context.resilience_context())
                    .timeout(Duration::from_secs(30))
                    .http_timeout_error(),
                Retry::layer("retry", context.resilience_context()).http_configure_defaults(),
                Timeout::layer("attempt_timeout", context.resilience_context())
                    .timeout(Duration::from_secs(10))
                    .http_timeout_error(),
                Logging::layer(context.clock(), context.redaction_engine()),
                Metrics::layer(context.clock()).meter_provider(opentelemetry::global::meter_provider().as_ref()),
                dispatch,
            );

            stack.into_service()
        })
        .build();
    let custom_standard_allocs = session.operation("custom_standard_pipeline");
    group.bench_function("custom_standard_pipeline", |b| {
        b.iter(|| {
            let _measure = custom_standard_allocs.measure_thread();
            _ = block_on(client.get(get_uri()).fetch()).unwrap();
        });
    });

    group.finish();
    session.print_to_stdout();
}

criterion_group!(benches, entry);
criterion_main!(benches);
