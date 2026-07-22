// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(missing_docs, reason = "benchmark code")]
use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use seatbelt::breaker::{Breaker, BreakerId};
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("breaker");
    let session = Session::new();

    // No circuit breaker
    let service = Execute::new(|_input: Input| async move { Output });
    let operation = session.operation("no-breaker");
    group.bench_function("no-breaker", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input(0)));
        });
    });

    // With circuit breaker (closed state)
    let context = ResilienceContext::new(Clock::new_frozen());

    let service = (
        Breaker::layer("bench", &context)
            .recovery_with(|_, _| RecoveryInfo::never())
            .rejected_input_error(|_input, _args| Output)
            .min_throughput(1000), // High threshold to keep circuit closed
        Execute::new(|_input: Input| async move { Ok(Output) }),
    )
        .into_service();

    let operation = session.operation("with-breaker");
    group.bench_function("with-breaker", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input(0)));
        });
    });

    // Partitioned breaker, single partition: every request targets the same authority
    // (the common case). Warmed so the measured loop only resolves the existing engine.
    let service = warmed_partitioned(0..1);
    let operation = session.operation("with-partitioned");
    group.bench_function("with-partitioned", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input(0)));
        });
    });

    // Partitioned breaker with a moderate number of engines already created.
    let service = warmed_partitioned(0..16);
    let operation = session.operation("with-partitioned-many");
    group.bench_function("with-partitioned-many", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input(0)));
        });
    });

    // Partitioned breaker with a large, high-cardinality engine set (worst case).
    let service = warmed_partitioned(0..256);
    let operation = session.operation("with-partitioned-large");
    group.bench_function("with-partitioned-large", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input(0)));
        });
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);

fn build_partitioned() -> impl Service<Input, Out = Result<Output, Output>> + 'static {
    let context = ResilienceContext::new(Clock::new_frozen());
    (
        Breaker::layer("bench", &context)
            .breaker_id(|input: &Input| BreakerId::from(input.0))
            .recovery_with(|_, _| RecoveryInfo::never())
            .rejected_input_error(|_input, _args| Output)
            .min_throughput(1000),
        Execute::new(|_input: Input| async move { Ok(Output) }),
    )
        .into_service()
}

// Builds a partitioned breaker and creates an engine for each partition in `partitions`.
// The measured loops always resolve partition 0.
fn warmed_partitioned(partitions: impl IntoIterator<Item = u64>) -> impl Service<Input, Out = Result<Output, Output>> + 'static {
    let service = build_partitioned();
    for partition in partitions {
        _ = block_on(service.execute(Input(partition)));
    }
    service
}

#[derive(Debug, Clone)]
struct Input(u64);

#[derive(Debug, Clone)]
struct Output;
