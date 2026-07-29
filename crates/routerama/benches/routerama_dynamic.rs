// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Focused wall-clock benchmarks for runtime-only router behavior.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "resolved benchmark variants are consumed through black_box")]

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

include!("common/dynamic_scenarios.rs");

fn dynamic_routes(c: &mut Criterion) {
    let typed = build_dynamic_typed();
    let mut typed_group = c.benchmark_group("routerama_dynamic/typed");
    typed_group.bench_function("unit", |b| b.iter(|| dynamic_typed_unit(&typed)));
    typed_group.bench_function("parse", |b| b.iter(|| dynamic_typed_parse(&typed)));
    typed_group.bench_function("owned_plain", |b| {
        b.iter(|| dynamic_typed_owned_plain(&typed));
    });
    typed_group.bench_function("owned_percent", |b| {
        b.iter(|| dynamic_typed_owned_percent(&typed));
    });
    typed_group.finish();

    let mut fanout = c.benchmark_group("routerama_dynamic/fanout");
    for width in [1, 2, 4, 8, 16, 32, 64] {
        let scenario = build_dynamic_fanout(width);
        fanout.bench_with_input(BenchmarkId::from_parameter(width), &scenario, |b, scenario| {
            b.iter(|| dynamic_fanout_lookup(scenario));
        });
    }
    fanout.finish();

    let mut capture_count = c.benchmark_group("routerama_dynamic/capture_count");
    for count in [4, 5] {
        let scenario = build_capture_threshold(count);
        capture_count.bench_with_input(BenchmarkId::from_parameter(count), &scenario, |b, scenario| {
            b.iter(|| dynamic_capture_threshold_lookup(scenario));
        });
    }
    capture_count.finish();

    let misses_router = build_dynamic_misses();
    let mut misses = c.benchmark_group("routerama_dynamic/misses");
    misses.bench_function("early", |b| b.iter(|| dynamic_early_miss(&misses_router)));
    misses.bench_function("late", |b| b.iter(|| dynamic_late_miss(&misses_router)));
    misses.bench_function("wrong_method", |b| {
        b.iter(|| dynamic_wrong_method(&misses_router));
    });
    misses.finish();

    let features_router = build_dynamic_features();
    let no_verb = build_dynamic_no_verb();
    let with_verb = build_dynamic_with_verb();
    let mut features = c.benchmark_group("routerama_dynamic/features");
    features.bench_function("rest", |b| b.iter(|| dynamic_rest(&features_router)));
    features.bench_function("affix", |b| b.iter(|| dynamic_affix(&features_router)));
    features.bench_function("no_verb_table", |b| b.iter(|| dynamic_no_verb(&no_verb)));
    features.bench_function("verb_table_nonverb_hit", |b| {
        b.iter(|| dynamic_with_verb_nonverb_hit(&with_verb));
    });
    features.bench_function("verb_hit", |b| b.iter(|| dynamic_verb_hit(&with_verb)));
    features.finish();

    let depth_16 = build_dynamic_depth(16);
    let depth_17 = build_dynamic_depth(17);
    let mut segment_depth = c.benchmark_group("routerama_dynamic/segment_depth");
    segment_depth.bench_function("shallow_in_16_table", |b| {
        b.iter(|| dynamic_depth_table_shallow_lookup(&depth_16));
    });
    segment_depth.bench_function("shallow_in_17_table", |b| {
        b.iter(|| dynamic_depth_table_shallow_lookup(&depth_17));
    });
    segment_depth.bench_function("deep_16", |b| {
        b.iter(|| dynamic_depth_table_deep_lookup(&depth_16));
    });
    segment_depth.bench_function("deep_17", |b| {
        b.iter(|| dynamic_depth_table_deep_lookup(&depth_17));
    });
    segment_depth.finish();

    let deep = build_deep_dynamic();
    let mut scratch = c.benchmark_group("routerama_dynamic/deep_scratch");
    scratch.bench_function("shallow_lookup", |b| {
        b.iter(|| dynamic_deep_table_shallow_lookup(&deep));
    });
    scratch.bench_function("deep_lookup", |b| {
        b.iter(|| dynamic_deep_table_deep_lookup(&deep));
    });
    scratch.finish();
}

criterion_group!(benches, dynamic_routes);
criterion_main!(benches);
