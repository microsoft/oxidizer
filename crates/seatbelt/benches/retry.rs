// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use oxidizer_benchmarking::BenchmarkGroupExt;
use seatbelt::retry::Retry;
use seatbelt::{RecoveryInfo, SeatbeltOptions};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

pub fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("retry");
    let session = Session::new();

    // No retries
    let service = Execute::new(|v: Input| async move { Output::from(v) });
    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "no-retry",
        &session,
    );

    // With retry
    let options = SeatbeltOptions::new(Clock::new_frozen());

    let service = (
        Retry::layer("bench", &options)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::never()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();

    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "with-retry",
        &session,
    );

    // With retry and recovery
    let options = SeatbeltOptions::new(Clock::new_frozen());

    let service = (
        Retry::layer("bench", &options)
            .clone_input()
            .max_retry_attempts(1)
            .base_delay(Duration::ZERO)
            .recovery_with(|_, _| RecoveryInfo::retry()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();

    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "with-retry-and-recovery",
        &session,
    );

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
