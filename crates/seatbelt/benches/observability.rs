// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service};
use opentelemetry::metrics::Counter;
use opentelemetry::{KeyValue, StringValue};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{SdkMeterProvider, Temporality};
use oxidizer_benchmarking::BenchmarkGroupExt;
use seatbelt::SeatbeltOptions;
use seatbelt::telemetry::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

pub fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("observability");
    let session = Session::new();

    // No observability
    let service = NoObservability(Execute::new(|v: Input| async move { Output::from(v) }));
    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "no-observability",
        &session,
    );

    // With observability
    let options = SeatbeltOptions::new(Clock::new_frozen());
    let service = Observability::new(
        &options,
        Execute::new(|v: Input| async move { Output::from(v) }),
    );
    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "observability",
        &session,
    );

    // With observability + listener
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(EmptyExporter)
        .build();
    let options = SeatbeltOptions::new(Clock::new_frozen()).meter_provider(&meter_provider);
    let service = Observability::new(
        &options,
        Execute::new(|v: Input| async move { Output::from(v) }),
    );
    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "observability-and-listener",
        &session,
    );

    session.print_to_stdout();
}

criterion_group!(benches, entry);
criterion_main!(benches);

struct NoObservability<S>(S);

impl<S> Service<Input> for NoObservability<S>
where
    S: Service<Input, Out = Output>,
{
    type Out = Output;

    async fn execute(&self, input: Input) -> Self::Out {
        self.0.execute(input).await
    }
}

struct Observability<S> {
    service: S,
    event_reporter: Counter<u64>,
    pipeline_name: StringValue,
}

impl<S> Observability<S> {
    pub fn new(options: &SeatbeltOptions<Input, Output>, service: S) -> Self {
        Self {
            service,
            event_reporter: options.create_resilience_event_counter(),
            pipeline_name: options.get_pipeline_name().clone().into(),
        }
    }
}

impl<S> Service<Input> for Observability<S>
where
    S: Service<Input, Out = Output>,
{
    type Out = Output;

    async fn execute(&self, input: Input) -> Self::Out {
        let output = self.service.execute(input).await;

        self.event_reporter.add(
            1,
            &[
                KeyValue::new(PIPELINE_NAME, self.pipeline_name.clone()),
                KeyValue::new(STRATEGY_NAME, "benchmark.observability"),
                KeyValue::new(EVENT_NAME, "custom"),
            ],
        );

        output
    }
}

struct Input;

struct Output;

impl From<Input> for Output {
    fn from(_input: Input) -> Self {
        Self
    }
}

struct EmptyExporter;

impl PushMetricExporter for EmptyExporter {
    async fn export(&self, _metrics: &ResourceMetrics) -> OTelSdkResult {
        Ok(())
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }

    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}
