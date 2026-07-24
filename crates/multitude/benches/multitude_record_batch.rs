// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock benchmarks for a synthetic wide-record batch
//! deserialization workload.
//!
//! Paired with `multitude_record_batch_cg.rs`, which covers representative
//! deterministic hot paths under Callgrind.

#![allow(dead_code, reason = "wide deserialized records are consumed as whole values")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]

use std::alloc::System;
#[cfg(feature = "stats")]
use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};

#[path = "multitude_record_batch/shared.rs"]
mod shared;

#[cfg(feature = "stats")]
use shared::diagnostic_stats;
use shared::{
    arena_box_slice_hot_path, arena_each_refresh_iteration, arena_each_refresh_state, arena_raw_each_refresh_iteration,
    arena_raw_each_refresh_state, arena_vec_baseline_hot_path, arena_vec_refresh_iteration, arena_vec_refresh_state,
    malformed_arena_hot_path, malformed_json, malformed_standard_hot_path, repeated_no_reset_iteration, reset_recreate_hot_path,
    reset_recreate_state, resource_limited_hot_path, reusable_vector_state, sparse_arena_hot_path, sparse_lazy_standard_hot_path,
    sparse_standard_hot_path, standard_refresh_iteration, standard_refresh_state, standard_vec_hot_path, warm_arena, workload_json,
};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn decode(criterion: &mut Criterion) {
    let input = workload_json(false);
    let allocations = Session::new();
    let standard_allocations = allocations.operation("multitude_record_batch_standard_vec");
    let box_allocations = allocations.operation("multitude_record_batch_arena_box_slice");
    let vec_allocations = allocations.operation("multitude_record_batch_arena_vec_baseline");
    let mut group = criterion.benchmark_group("multitude_record_batch/decode");

    group.bench_function("standard_vec", |bencher| {
        bencher.iter(|| {
            let _span = standard_allocations.measure_thread();
            standard_vec_hot_path(&input);
        });
    });
    group.bench_function("arena_box_slice", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| {
            let _span = box_allocations.measure_thread();
            arena_box_slice_hot_path(&arena, &input);
        });
    });
    group.bench_function("arena_vec_baseline", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| {
            let _span = vec_allocations.measure_thread();
            arena_vec_baseline_hot_path(&arena, &input);
        });
    });
    group.finish();
}

fn strings(criterion: &mut Criterion) {
    let unescaped = workload_json(false);
    let escaped = workload_json(true);
    let mut group = criterion.benchmark_group("multitude_record_batch/strings");

    group.bench_function("standard_vec_unescaped", |bencher| {
        bencher.iter(|| standard_vec_hot_path(&unescaped));
    });
    group.bench_function("standard_vec_escaped", |bencher| bencher.iter(|| standard_vec_hot_path(&escaped)));
    group.bench_function("arena_vec_unescaped", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| arena_vec_baseline_hot_path(&arena, &unescaped));
    });
    group.bench_function("arena_vec_escaped", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| arena_vec_baseline_hot_path(&arena, &escaped));
    });
    group.finish();
}

fn reuse(criterion: &mut Criterion) {
    let allocations = Session::new();
    let repeated_allocations = allocations.operation("multitude_record_batch_repeated_no_reset");
    let reset_allocations = allocations.operation("multitude_record_batch_reset_recreate");
    let mut group = criterion.benchmark_group("multitude_record_batch/reuse");

    group.bench_function("repeated_no_reset", |bencher| {
        let mut state = reusable_vector_state();
        bencher.iter(|| {
            let _span = repeated_allocations.measure_thread();
            repeated_no_reset_iteration(&mut state);
        });
    });
    group.bench_function("reset_recreate", |bencher| {
        let mut state = reset_recreate_state();
        bencher.iter(|| {
            let _span = reset_allocations.measure_thread();
            reset_recreate_hot_path(&mut state.arena, &state.input);
        });
    });
    group.finish();
}

fn sparse_retention(criterion: &mut Criterion) {
    let input = workload_json(false);
    let mut group = criterion.benchmark_group("multitude_record_batch/sparse_retention");

    group.bench_function("standard_one_in_eight", |bencher| bencher.iter(|| sparse_standard_hot_path(&input)));
    group.bench_function("arena_one_in_eight", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| sparse_arena_hot_path(&arena, &input));
    });
    group.finish();
}

fn lazy_raw_strings(criterion: &mut Criterion) {
    let escaped = workload_json(true);
    let mut group = criterion.benchmark_group("multitude_record_batch/lazy_raw_strings");

    group.bench_function("eager_sparse_escaped", |bencher| {
        bencher.iter(|| sparse_standard_hot_path(&escaped));
    });
    group.bench_function("lazy_sparse_escaped", |bencher| {
        bencher.iter(|| sparse_lazy_standard_hot_path(&escaped));
    });
    group.finish();
}

fn errors(criterion: &mut Criterion) {
    let malformed = malformed_json();
    let valid = workload_json(false);
    let mut group = criterion.benchmark_group("multitude_record_batch/errors");

    group.bench_function("malformed_standard", |bencher| {
        bencher.iter(|| malformed_standard_hot_path(&malformed));
    });
    group.bench_function("malformed_arena", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| malformed_arena_hot_path(&arena, &malformed));
    });
    group.bench_function("resource_limited_arena", |bencher| {
        let arena = warm_arena();
        bencher.iter(|| resource_limited_hot_path(&arena, &valid));
    });
    group.finish();
}

#[cfg(feature = "stats")]
fn diagnostics(criterion: &mut Criterion) {
    let input = workload_json(false);
    let (arena, live, released) = diagnostic_stats(&input);
    eprintln!("multitude_record_batch ArenaStats with live batch: {live:?}; after drop: {released:?}");

    let mut group = criterion.benchmark_group("multitude_record_batch/diagnostics");
    group.bench_function("arena_stats_snapshot", |bencher| bencher.iter(|| black_box(arena.stats())));
    group.finish();
}

#[cfg(not(feature = "stats"))]
fn diagnostics(_: &mut Criterion) {}

fn refresh_workload(criterion: &mut Criterion) {
    let allocations = Session::new();
    let standard_allocations = allocations.operation("multitude_record_batch_refresh_standard");
    let vector_allocations = allocations.operation("multitude_record_batch_refresh_arena_vec");
    let streaming_allocations = allocations.operation("multitude_record_batch_refresh_arena_each");
    let raw_streaming_allocations = allocations.operation("multitude_record_batch_refresh_arena_raw_each");
    let mut group = criterion.benchmark_group("multitude_record_batch/refresh_workload");
    group.sample_size(20);

    group.bench_function("standard_selective", |bencher| {
        let mut state = standard_refresh_state();
        bencher.iter(|| {
            let _span = standard_allocations.measure_thread();
            standard_refresh_iteration(&mut state);
        });
    });
    group.bench_function("arena_vec_reset_selective", |bencher| {
        let mut state = arena_vec_refresh_state();
        bencher.iter(|| {
            let _span = vector_allocations.measure_thread();
            arena_vec_refresh_iteration(&mut state);
        });
    });
    group.bench_function("arena_each_reset_selective", |bencher| {
        let mut state = arena_each_refresh_state();
        bencher.iter(|| {
            let _span = streaming_allocations.measure_thread();
            arena_each_refresh_iteration(&mut state);
        });
    });
    group.bench_function("arena_raw_each_reset_index_selective", |bencher| {
        let mut state = arena_raw_each_refresh_state();
        bencher.iter(|| {
            let _span = raw_streaming_allocations.measure_thread();
            arena_raw_each_refresh_iteration(&mut state);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    decode,
    strings,
    reuse,
    sparse_retention,
    lazy_raw_strings,
    errors,
    diagnostics,
    refresh_workload
);
criterion_main!(benches);
