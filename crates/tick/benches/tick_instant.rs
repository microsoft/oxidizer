// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares standard and lower-resolution instant retrieval.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use tick::SimpleClock;

fn criterion_benchmark(c: &mut Criterion) {
    retrieval(c);
}

fn retrieval(c: &mut Criterion) {
    let precise_clock = SimpleClock::new_system();
    let fast_clock = precise_clock.clone().with_fast_instant(true);
    let mut group = c.benchmark_group("tick_instant/retrieval");

    group.bench_function("instant", |b| {
        b.iter(|| black_box(precise_clock.instant()));
    });
    group.bench_function("instant_fast", |b| {
        b.iter(|| black_box(fast_clock.instant()));
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
