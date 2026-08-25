// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock benchmarks for releasing allocations.
//!
//! Paired with `multitude_teardown_cg.rs`, which measures the same hot paths
//! under Callgrind.

#![allow(clippy::unwrap_used, reason = "benchmark code")]

use std::alloc::System;
use std::time::{Duration, Instant};

use alloc_tracker::{Allocator as TrackingAllocator, Session};
use criterion::{Bencher, Criterion, criterion_group, criterion_main};

#[path = "multitude_teardown/shared.rs"]
mod shared;

use shared::{
    LARGE, MEDIUM, SMALL, bumpalo_state, free_standard, multitude_state, reset_allocate_bumpalo, reset_allocate_multitude, reset_bumpalo,
    reset_multitude, standard_state,
};

#[global_allocator]
static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();

const INPUTS_PER_BATCH: u64 = 16;

fn iter_with_setup<T>(bencher: &mut Bencher<'_>, mut setup: impl FnMut() -> T, mut routine: impl FnMut(&mut T)) {
    bencher.iter_custom(|iters| {
        let mut remaining = iters;
        let mut elapsed = Duration::ZERO;

        while remaining != 0 {
            let batch_len = remaining.min(INPUTS_PER_BATCH);
            let mut inputs = (0..batch_len).map(|_| setup()).collect::<Vec<_>>();
            let start = Instant::now();
            for input in &mut inputs {
                routine(input);
            }
            elapsed += start.elapsed();
            drop(inputs);
            remaining -= batch_len;
        }

        elapsed
    });
}

fn assert_allocation_free<T>(name: &str, mut input: T, routine: impl FnOnce(&mut T)) {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation(name);
    {
        let _measurement = operation.measure_thread().iterations(1);
        routine(&mut input);
    }
    drop(input);

    let report = session.to_report();
    let (_, metrics) = report
        .operations()
        .find(|(operation_name, _)| *operation_name == name)
        .expect("allocation operation was registered immediately above");
    assert_eq!(
        metrics.total_allocations_count(),
        0,
        "{name} unexpectedly called the backing allocator"
    );
    assert_eq!(metrics.total_bytes_allocated(), 0, "{name} unexpectedly allocated backing bytes");
}

fn bench_count<const N: usize>(criterion: &mut Criterion, name: &str) {
    assert_allocation_free(&format!("{name}/multitude_reset"), multitude_state::<N>(), reset_multitude);
    assert_allocation_free(&format!("{name}/bumpalo_reset"), bumpalo_state::<N>(), reset_bumpalo);
    assert_allocation_free(
        &format!("{name}/multitude_reset_allocate"),
        multitude_state::<N>(),
        reset_allocate_multitude,
    );
    assert_allocation_free(
        &format!("{name}/bumpalo_reset_allocate"),
        bumpalo_state::<N>(),
        reset_allocate_bumpalo,
    );

    let mut group = criterion.benchmark_group(format!("multitude_teardown/{name}"));
    group.bench_function("standard", |bencher| {
        iter_with_setup(bencher, standard_state::<N>, free_standard);
    });
    group.bench_function("multitude", |bencher| {
        iter_with_setup(bencher, multitude_state::<N>, reset_multitude);
    });
    group.bench_function("bumpalo", |bencher| {
        iter_with_setup(bencher, bumpalo_state::<N>, reset_bumpalo);
    });
    group.bench_function("multitude_reset_allocate", |bencher| {
        iter_with_setup(bencher, multitude_state::<N>, reset_allocate_multitude);
    });
    group.bench_function("bumpalo_reset_allocate", |bencher| {
        iter_with_setup(bencher, bumpalo_state::<N>, reset_allocate_bumpalo);
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
