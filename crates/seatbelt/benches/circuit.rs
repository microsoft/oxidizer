// Copyright (c) Microsoft Corporation.

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use seatbelt::circuit::Circuit;
use seatbelt::{RecoveryInfo, SeatbeltOptions};
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("circuit_breaker");
    let session = Session::new();

    // No circuit breaker
    let service = Execute::new(|_input: Input| async move { Output });
    let operation = session.operation("no-circuit-breaker");
    group.bench_function("no-circuit-breaker", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // With circuit breaker (closed state)
    let options = SeatbeltOptions::new(Clock::new_frozen());

    let service = (
        Circuit::layer("bench", &options)
            .recovery_with(|_, _| RecoveryInfo::never())
            .rejected_input_error(|_input, _args| Output)
            .min_throughput(1000), // High threshold to keep circuit closed
        Execute::new(|_input: Input| async move { Ok(Output) }),
    )
        .build();

    let operation = session.operation("with-circuit-breaker");
    group.bench_function("with-circuit-breaker", |b| {
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
