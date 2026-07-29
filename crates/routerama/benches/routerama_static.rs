// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Focused wall-clock benchmarks for generated static resolver branches.
//!
//! Paired with `routerama_static_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared scenario module supports both benchmark harnesses")]

use criterion::{Criterion, criterion_group, criterion_main};

include!("common/static_scenarios.rs");

fn static_routes(c: &mut Criterion) {
    let router = build_static_scenario();

    let mut hits = c.benchmark_group("routerama_static/hits");
    hits.bench_function("shallow_literal", |b| b.iter(|| static_shallow_literal(&router)));
    hits.bench_function("deep_literal", |b| b.iter(|| static_deep_literal(&router)));
    hits.bench_function("fanout_first", |b| b.iter(|| static_fanout_first(&router)));
    hits.bench_function("fanout_middle", |b| b.iter(|| static_fanout_middle(&router)));
    hits.bench_function("fanout_last", |b| b.iter(|| static_fanout_last(&router)));
    hits.bench_function("borrow_one", |b| b.iter(|| static_borrow_one(&router)));
    hits.bench_function("borrow_four", |b| b.iter(|| static_borrow_four(&router)));
    hits.bench_function("parse_number", |b| b.iter(|| static_parse_number(&router)));
    hits.bench_function("own_plain", |b| b.iter(|| static_own_plain(&router)));
    hits.bench_function("own_percent", |b| b.iter(|| static_own_percent(&router)));
    hits.finish();

    let mut misses = c.benchmark_group("routerama_static/misses");
    misses.bench_function("early", |b| b.iter(|| static_early_miss(&router)));
    misses.bench_function("late", |b| b.iter(|| static_late_miss(&router)));
    misses.bench_function("pathological_long", |b| {
        b.iter(|| static_pathological_long_miss(&router));
    });
    misses.bench_function("wrong_method", |b| b.iter(|| static_wrong_method(&router)));
    misses.finish();

    let no_verb = build_no_verb_scenario();
    let with_verb = build_with_verb_scenario();
    let mut features = c.benchmark_group("routerama_static/features");
    features.bench_function("rest", |b| b.iter(|| static_rest(&router)));
    features.bench_function("affix", |b| b.iter(|| static_affix(&router)));
    features.bench_function("no_verb_table", |b| b.iter(|| static_no_verb(&no_verb)));
    features.bench_function("verb_table_nonverb_hit", |b| {
        b.iter(|| static_with_verb_nonverb_hit(&with_verb));
    });
    features.bench_function("verb_hit", |b| b.iter(|| static_with_verb_hit(&with_verb)));
    features.finish();

    let shallow = build_shallow_table();
    let deep_outlier = build_deep_outlier_table();
    let mut shape = c.benchmark_group("routerama_static/table_shape");
    shape.bench_function("shallow_table_hit", |b| {
        b.iter(|| static_shallow_table_hit(&shallow));
    });
    shape.bench_function("deep_outlier_table_hit", |b| {
        b.iter(|| static_deep_outlier_table_hit(&deep_outlier));
    });
    let affix_fanout = build_affix_fanout();
    shape.bench_function("affix_fanout_first", |b| {
        b.iter(|| static_affix_fanout_first(&affix_fanout));
    });
    shape.bench_function("affix_fanout_middle", |b| {
        b.iter(|| static_affix_fanout_middle(&affix_fanout));
    });
    shape.bench_function("affix_fanout_last", |b| {
        b.iter(|| static_affix_fanout_last(&affix_fanout));
    });
    shape.finish();
}

criterion_group!(benches, static_routes);
criterion_main!(benches);
