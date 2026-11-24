// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::missing_panics_doc, clippy::wildcard_imports, reason = "benchmark readability")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![expect(missing_docs, reason = "benchmark code")]

use std::hint::black_box;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample;
use criterion::{Criterion, criterion_group, criterion_main};
use data_privacy::RedactionEngine;
use fetch::HttpClient;
use fetch::fake::FakeDeps;
use fetch::resilience::retry::HttpRetryLayerExt;
use fetch::resilience::timeout::HttpTimeoutLayerExt;
use futures::executor::block_on;
use http::StatusCode;
use layered::Stack;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView, FieldValueFn};
use observed::{FlushError, Sink, SinkId};
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use tick::{Clock, SimpleClock};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const URI_STRING: &str = "https://example.com/some/path?query=value";

fn get_uri() -> &'static str {
    URI_STRING
}

/// Reads each field and each enrichment of every event, and keeps nothing. Thus the measurements
/// do not include the work of keeping the events.
struct ReadAll(RedactionEngine);

impl EventProcessor for ReadAll {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let _ = event.visit_fields(&mut |_: &FieldDescriptor, value: &FieldValueFn<'_>| {
            black_box(value(&self.0));
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |_: &FieldDescriptor, value: &FieldValueFn<'_>| {
            black_box(value(&self.0));
            ControlFlow::Continue(())
        });
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn entry(c: &mut Criterion) {
    let session = Session::new();
    let mut group = c.benchmark_group("http_client_pipelines");

    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), Sink::noop())).build();
    let standard_allocs = session.operation("standard_pipeline");
    group.bench_function("standard_pipeline", |b| {
        b.iter_custom(|iters| {
            let _measure = standard_allocs.measure_thread().iterations(iters);
            time_sample(iters, || block_on(client.get(get_uri()).fetch()).unwrap())
        });
    });

    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), Sink::noop()))
        .minimal_pipeline()
        .build();
    let minimal_allocs = session.operation("minimal_pipeline");
    group.bench_function("minimal_pipeline", |b| {
        b.iter_custom(|iters| {
            let _measure = minimal_allocs.measure_thread().iterations(iters);
            time_sample(iters, || block_on(client.get(get_uri()).fetch()).unwrap())
        });
    });

    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), Sink::noop()))
        .custom_pipeline(|dispatch, _context| dispatch)
        .build();
    let custom_minimal_allocs = session.operation("custom_minimal_pipeline");
    group.bench_function("custom_minimal_pipeline", |b| {
        b.iter_custom(|iters| {
            let _measure = custom_minimal_allocs.measure_thread().iterations(iters);
            time_sample(iters, || block_on(client.get(get_uri()).fetch()).unwrap())
        });
    });

    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), Sink::noop()))
        .custom_pipeline(|dispatch, context| {
            let stack = (
                Timeout::layer("total_timeout", context.resilience_context())
                    .timeout(Duration::from_secs(30))
                    .http_timeout_error(),
                Retry::layer("retry", context.resilience_context()).http_configure_defaults(),
                Timeout::layer("attempt_timeout", context.resilience_context())
                    .timeout(Duration::from_secs(10))
                    .http_timeout_error(),
                context.client_telemetry(),
                dispatch,
            );

            stack.into_service()
        })
        .build();
    let custom_standard_allocs = session.operation("custom_standard_pipeline");
    group.bench_function("custom_standard_pipeline", |b| {
        b.iter_custom(|iters| {
            let _measure = custom_standard_allocs.measure_thread().iterations(iters);
            time_sample(iters, || block_on(client.get(get_uri()).fetch()).unwrap())
        });
    });

    let sink = Sink::builder(SinkId::new("reading"))
        .clock(SimpleClock::new_frozen())
        .processors(vec![Arc::new(ReadAll(observed_testing::passthrough_redaction_engine()))])
        .build();
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::with_clock(&Clock::new_frozen(), sink)).build();
    // At the first event of a sink on a thread, `observed` allocates storage for that thread. The
    // request before the measurement keeps that storage out of the numbers.
    block_on(client.get(get_uri()).fetch()).unwrap();
    let standard_observed_allocs = session.operation("standard_pipeline_observed");
    group.bench_function("standard_pipeline_observed", |b| {
        b.iter_custom(|iters| {
            let _measure = standard_observed_allocs.measure_thread().iterations(iters);
            time_sample(iters, || block_on(client.get(get_uri()).fetch()).unwrap())
        });
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);
