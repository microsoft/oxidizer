// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use oxidizer_benchmarking::BenchmarkGroupExt;
use seatbelt::SeatbeltOptions;
use seatbelt::timeout::Timeout;
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

pub fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("timeout");
    let session = Session::new();

    // No timeout
    let service = Execute::new(|v: Input| async move { Output::from(v) });
    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "no-timeout",
        &session,
    );

    // With timeout
    let options = SeatbeltOptions::new(Clock::new_frozen());

    let service = (
        Timeout::layer("bench", &options)
            .timeout_output(|_args| Output)
            .timeout(Duration::from_secs(10)),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();

    group.bench_with_memory(
        || _ = block_on(service.execute(Input)),
        "with-timeout",
        &session,
    );

    session.print_to_stdout();
}

criterion_group!(benches, entry);
criterion_main!(benches);

struct Input;

struct Output;

impl From<Input> for Output {
    fn from(_input: Input) -> Self {
        Self
    }
}
