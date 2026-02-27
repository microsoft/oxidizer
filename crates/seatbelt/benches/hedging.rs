// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(missing_docs, reason = "benchmark code")]

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use seatbelt::hedging::Hedging;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("hedging");
    let session = Session::new();

    // No hedging (baseline)
    let service = Execute::new(|v: Input| async move { Output::from(v) });
    let operation = session.operation("no-hedging");
    group.bench_function("no-hedging", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // With hedging (delay, no recovery needed)
    let context = ResilienceContext::new(Clock::new_frozen());

    let service = (
        Hedging::layer("bench", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::never()),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .into_service();

    let operation = session.operation("with-hedging-delay");
    group.bench_function("with-hedging-delay", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // With hedging disabled (max_hedged_attempts = 0)
    let context = ResilienceContext::new(Clock::new_frozen());

    let service = (
        Hedging::layer("bench", &context)
            .clone_input()
            .recovery_with(|_, _| RecoveryInfo::never())
            .max_hedged_attempts(0),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .into_service();

    let operation = session.operation("with-hedging-passthrough");
    group.bench_function("with-hedging-passthrough", |b| {
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
