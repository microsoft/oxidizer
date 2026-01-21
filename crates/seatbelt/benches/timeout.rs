// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(missing_docs, reason = "benchmark code")]

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Service, Stack};
use seatbelt::PipelineContext;
use seatbelt::timeout::Timeout;
use tick::Clock;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("timeout");
    let session = Session::new();

    // No timeout
    let service = Execute::new(|v: Input| async move { Output::from(v) });
    let operation = session.operation("no-timeout");
    group.bench_function("no-timeout", |b| {
        b.iter(|| {
            let _span = operation.measure_thread();
            _ = block_on(service.execute(Input));
        });
    });

    // With timeout
    let context = PipelineContext::new(Clock::new_frozen());

    let service = (
        Timeout::layer("bench", &context)
            .timeout_output(|_args| Output)
            .timeout(Duration::from_secs(10)),
        Execute::new(|v: Input| async move { Output::from(v) }),
    )
        .build();

    let operation = session.operation("with-timeout");
    group.bench_function("with-timeout", |b| {
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

struct Input;

struct Output;

impl From<Input> for Output {
    fn from(_input: Input) -> Self {
        Self
    }
}
