// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares standard and lower-resolution instant retrieval.

use std::borrow::Borrow;
use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};
use tick::{SimpleClock, Stopwatch};

const RETRIEVAL: &str = "tick_instant/retrieval";

fn started(mut stopwatch: Stopwatch) -> Stopwatch {
    stopwatch.start();
    stopwatch
}

#[metabench::benchmark(INSTANT, RETRIEVAL, "instant")]
#[bench::precise(Box::new(started(Stopwatch::new(SimpleClock::new_system()))))]
fn instant<C: Borrow<Stopwatch>>(stopwatch: C) -> (Duration, C) {
    let elapsed = black_box(stopwatch.borrow()).elapsed();
    (elapsed, stopwatch)
}

#[metabench::benchmark(INSTANT_COARSE, RETRIEVAL, "instant_coarse")]
#[bench::coarse(Box::new(started(Stopwatch::new_coarse(SimpleClock::new_system()))))]
fn instant_coarse<C: Borrow<Stopwatch>>(stopwatch: C) -> (Duration, C) {
    let elapsed = black_box(stopwatch.borrow()).elapsed();
    (elapsed, stopwatch)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let precise_stopwatch = started(Stopwatch::new(SimpleClock::new_system()));
    let coarse_stopwatch = started(Stopwatch::new_coarse(SimpleClock::new_system()));
    let mut group = criterion.benchmark_group(RETRIEVAL);

    group.bench_function(BenchmarkId::new(INSTANT.benchmark_name(), "precise"), |bencher| {
        bencher.iter(|| instant(black_box(&precise_stopwatch)).0);
    });
    group.bench_function(BenchmarkId::new(INSTANT_COARSE.benchmark_name(), "coarse"), |bencher| {
        bencher.iter(|| instant_coarse(black_box(&coarse_stopwatch)).0);
    });

    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [INSTANT, INSTANT_COARSE],);
