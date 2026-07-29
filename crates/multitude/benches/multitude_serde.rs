// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion benchmarks for arena-aware Serde deserialization.

#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(dead_code, reason = "deserialized benchmark records are consumed as whole values")]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

#[path = "multitude_serde/shared.rs"]
mod shared;

use shared::{
    StandardRecord, arena_output, batch_bumpalo_lifecycle, batch_multitude_lifecycle, batch_standard_lifecycle, dynamic_arena_hot_path,
    dynamic_standard_hot_path, typed_arena_hot_path, typed_bumpalo_lifecycle, typed_multitude_lifecycle, typed_standard_hot_path,
    typed_standard_lifecycle, warm_bump, warm_reset_arena,
};

fn typed(c: &mut Criterion) {
    let mut group = c.benchmark_group("multitude_serde/typed");
    group.bench_function("arena_owned", |bencher| {
        bencher.iter_batched_ref(arena_output, typed_arena_hot_path, BatchSize::PerIteration);
    });
    group.bench_function("serde_json_owned", |bencher| {
        bencher.iter_batched_ref(
            || None,
            |output: &mut Option<StandardRecord>| typed_standard_hot_path(output),
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn dynamic(c: &mut Criterion) {
    let mut group = c.benchmark_group("multitude_serde/dynamic");
    group.bench_function("arena_value", |bencher| {
        bencher.iter_batched_ref(arena_output, dynamic_arena_hot_path, BatchSize::PerIteration);
    });
    group.bench_function("serde_json_value", |bencher| {
        bencher.iter_batched_ref(
            || None,
            |output: &mut Option<serde_json::Value>| dynamic_standard_hot_path(output),
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn typed_lifecycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("multitude_serde/typed_lifecycle");
    group.bench_function("serde_json", |bencher| {
        let mut state = ();
        bencher.iter(|| typed_standard_lifecycle(&mut state));
    });
    group.bench_function("multitude", |bencher| {
        let mut arena = warm_reset_arena();
        bencher.iter(|| typed_multitude_lifecycle(&mut arena));
    });
    group.bench_function("bumpalo", |bencher| {
        let mut bump = warm_bump();
        bencher.iter(|| typed_bumpalo_lifecycle(&mut bump));
    });
    group.finish();
}

fn batch_lifecycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("multitude_serde/batch_lifecycle");
    group.bench_function("serde_json", |bencher| {
        let mut state = ();
        bencher.iter(|| batch_standard_lifecycle(&mut state));
    });
    group.bench_function("multitude", |bencher| {
        let mut arena = warm_reset_arena();
        bencher.iter(|| batch_multitude_lifecycle(&mut arena));
    });
    group.bench_function("bumpalo", |bencher| {
        let mut bump = warm_bump();
        bencher.iter(|| batch_bumpalo_lifecycle(&mut bump));
    });
    group.finish();
}

criterion_group!(benches, typed, dynamic, typed_lifecycle, batch_lifecycle);
criterion_main!(benches);
