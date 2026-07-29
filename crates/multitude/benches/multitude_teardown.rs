// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock benchmarks for releasing allocations.
//!
//! Paired with `multitude_teardown_cg.rs`, which measures the same hot paths
//! under Callgrind.

#![allow(clippy::unwrap_used, reason = "benchmark code")]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

#[path = "multitude_teardown/shared.rs"]
mod shared;

use shared::{LARGE, MEDIUM, SMALL, bumpalo_state, free_standard, multitude_state, reset_bumpalo, reset_multitude, standard_state};

fn bench_count<const N: usize>(criterion: &mut Criterion, name: &str) {
    let mut group = criterion.benchmark_group(format!("multitude_teardown/{name}"));
    group.bench_function("standard", |bencher| {
        bencher.iter_batched_ref(standard_state::<N>, free_standard, BatchSize::SmallInput);
    });
    group.bench_function("multitude", |bencher| {
        bencher.iter_batched_ref(multitude_state::<N>, reset_multitude, BatchSize::SmallInput);
    });
    group.bench_function("bumpalo", |bencher| {
        bencher.iter_batched_ref(bumpalo_state::<N>, reset_bumpalo, BatchSize::SmallInput);
    });
    group.finish();
}

fn benchmarks(criterion: &mut Criterion) {
    bench_count::<SMALL>(criterion, "free_1");
    bench_count::<MEDIUM>(criterion, "free_32");
    bench_count::<LARGE>(criterion, "free_1000");
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
