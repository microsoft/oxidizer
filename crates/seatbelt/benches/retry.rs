// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(missing_docs, reason = "benchmark code")]

use std::time::{Duration, Instant};

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use seatbelt::retry::Retry;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn time_sample<R>(mut bench: impl FnMut() -> R) -> impl FnMut(u64) -> Duration {
    move |iters| {
        let start = Instant::now();
        for _ in 0..iters {
            _ = std::hint::black_box(bench());
        }
        start.elapsed()
    }
}

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("retry");
    let session = Session::new();

    // No retries
    let service = Execute::new(|v: Input| async move { Output::from(v) });
    let operation = session.operation("no-retry");
    group.bench_function("no-retry", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(Input)))(iters)
        });
    });

    // With retry
    let context = ResilienceContext::new(Clock::new_frozen());

    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::never()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .into_service();

    let operation = session.operation("with-retry");
    group.bench_function("with-retry", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(Input)))(iters)
        });
    });

    // With retry and recovery
    let context = ResilienceContext::new(Clock::new_frozen());

    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .max_retry_attempts(1)
            .base_delay(Duration::ZERO)
            .recovery_with(|_, _| RecoveryInfo::retry()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .into_service();

    let operation = session.operation("with-retry-and-recovery");
    group.bench_function("with-retry-and-recovery", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(Input)))(iters)
        });
    });

    group.finish();
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
