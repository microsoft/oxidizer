// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares standard and lower-resolution instant retrieval.

use std::borrow::Borrow;
use std::hint::black_box;
use std::time::Instant;

use criterion::{BenchmarkId, Criterion};
use tick::SimpleClock;

const RETRIEVAL: &str = "tick_instant/retrieval";

#[metabench::benchmark(INSTANT, RETRIEVAL, "instant")]
#[bench::precise(Box::new(SimpleClock::new_system()))]
fn instant<C: Borrow<SimpleClock>>(clock: C) -> (Instant, C) {
    let instant = black_box(clock.borrow()).instant();
    (instant, clock)
}

#[metabench::benchmark(INSTANT_FAST, RETRIEVAL, "instant_fast")]
#[bench::fast(Box::new(SimpleClock::new_system().with_fast_instant(true)))]
fn instant_fast<C: Borrow<SimpleClock>>(clock: C) -> (Instant, C) {
    let instant = black_box(clock.borrow()).instant();
    (instant, clock)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let precise_clock = SimpleClock::new_system();
    let fast_clock = precise_clock.clone().with_fast_instant(true);
    let mut group = criterion.benchmark_group(RETRIEVAL);

    group.bench_function(BenchmarkId::new(INSTANT.benchmark_name(), "precise"), |bencher| {
        bencher.iter(|| instant(black_box(&precise_clock)).0);
    });
    group.bench_function(BenchmarkId::new(INSTANT_FAST.benchmark_name(), "fast"), |bencher| {
        bencher.iter(|| instant_fast(black_box(&fast_clock)).0);
    });

    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [INSTANT, INSTANT_FAST],);
