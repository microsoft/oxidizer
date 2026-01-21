// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(missing_docs, reason = "benchmark code")]

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use seatbelt::retry::Retry;
use seatbelt::{PipelineContext, RecoveryInfo};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("retry");
    let session = Session::new();

    // No retries
    let service = Execute::new(|v: Input| async move { Output::from(v) });
    let operation = session.operation("no-retry");
    group.bench_function("no-retry", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // With retry
    let context = PipelineContext::new(Clock::new_frozen());

    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::never()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();

    let operation = session.operation("with-retry");
    group.bench_function("with-retry", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // With retry and recovery
    let context = PipelineContext::new(Clock::new_frozen());

    let service = (
        Retry::layer("bench", &context)
            .clone_input()
            .max_retry_attempts(1)
            .base_delay(Duration::ZERO)
            .recovery_with(|_, _| RecoveryInfo::retry()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();

    let operation = session.operation("with-retry-and-recovery");
    group.bench_function("with-retry-and-recovery", |b| {
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
