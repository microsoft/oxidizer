// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares standard and lower-resolution instant retrieval.

use std::hint::black_box;
use std::time::Instant;

use criterion::Criterion;
use tick::SimpleClock;

const RETRIEVAL: &str = "tick_instant/retrieval";

#[metabench::benchmark(INSTANT, RETRIEVAL, "instant")]
#[bench::precise(&SimpleClock::new_system())]
fn instant(clock: &SimpleClock) -> Instant {
    black_box(clock).instant()
}

#[metabench::benchmark(INSTANT_FAST, RETRIEVAL, "instant_fast")]
#[bench::fast(&SimpleClock::new_system().with_fast_instant(true))]
fn instant_fast(clock: &SimpleClock) -> Instant {
    black_box(clock).instant()
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let precise_clock = SimpleClock::new_system();
    let fast_clock = precise_clock.clone().with_fast_instant(true);
    let mut group = criterion.benchmark_group(RETRIEVAL);

    group.bench_function(INSTANT.benchmark_name(), |bencher| {
        bencher.iter(|| instant(black_box(&precise_clock)));
    });
    group.bench_function(INSTANT_FAST.benchmark_name(), |bencher| {
        bencher.iter(|| instant_fast(black_box(&fast_clock)));
    });

    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [INSTANT, INSTANT_FAST],);
