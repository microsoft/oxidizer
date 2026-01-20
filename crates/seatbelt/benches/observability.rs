// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(missing_docs, reason = "benchmark code")]
use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{SdkMeterProvider, Temporality};
use seatbelt::retry::Retry;
use seatbelt::{Context, RecoveryInfo};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("observability");
    let session = Session::new();

    // No telemetry
    let context = Context::new(Clock::new_frozen());
    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .base_delay(Duration::ZERO)
            .recovery_with(|_, _| RecoveryInfo::retry()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();
    let operation = session.operation("retry-no-telemetry");
    group.bench_function("retry-no-telemetry", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // Metrics
    let meter_provider = SdkMeterProvider::builder().with_periodic_exporter(EmptyExporter).build();
    let context = Context::new(Clock::new_frozen()).enable_metrics(&meter_provider);
    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .base_delay(Duration::ZERO)
            .recovery_with(|_, _| RecoveryInfo::retry()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();
    let operation = session.operation("retry-metrics");
    group.bench_function("retry-metrics", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // Logs
    let context = Context::new(Clock::new_frozen()).enable_logs();
    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .base_delay(Duration::ZERO)
            .recovery_with(|_, _| RecoveryInfo::retry()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();
    let operation = session.operation("retry-logs");
    group.bench_function("retry-logs", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    group.finish();
    session.print_to_stdout();
}

criterion_group!(benches, entry);
criterion_main!(benches);

#[derive(Debug, Clone)]
struct Input;

#[derive(Debug, Clone)]
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
