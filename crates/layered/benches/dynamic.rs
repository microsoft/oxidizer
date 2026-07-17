// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "Benchmarks don't require documentation")]

use std::time::{Duration, Instant};

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{DynamicServiceExt, Execute, Intercept, Service, Stack};

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
    let mut group = c.benchmark_group("typed-vs-dynamic");
    let session = Session::new();

    let service = Execute::new(|v| async move { v });
    let operation = session.operation("typed");
    group.bench_function("typed", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(10)))(iters)
        });
    });

    let service = Execute::new(|v| async move { v }).into_dynamic();
    let operation = session.operation("dynamic");
    group.bench_function("dynamic", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(10)))(iters)
        });
    });

    let service = (Intercept::layer(), Execute::new(|v| async move { v })).into_service();
    let operation = session.operation("wrapped_typed");
    group.bench_function("wrapped_typed", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(10)))(iters)
        });
    });

    let service = (Intercept::layer(), Execute::new(|v| async move { v }))
        .into_service()
        .into_dynamic();
    let operation = session.operation("wrapped_dynamic");
    group.bench_function("wrapped_dynamic", |b| {
        b.iter_custom(|iters| {
            let _span = operation.measure_thread().iterations(iters);
            time_sample(|| block_on(service.execute(10)))(iters)
        });
    });
}

criterion_group!(benches, entry);
criterion_main!(benches);
