// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for mixed static and runtime routing.
//!
//! Paired with `routerama_mixed_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "resolved benchmark variants are consumed through black_box")]

use criterion::{Criterion, criterion_group, criterion_main};

include!("common/mixed_scenarios.rs");

fn mixed_routes(c: &mut Criterion) {
    let router = build_mixed_scenario();
    let mut dispatch = c.benchmark_group("routerama_mixed/dispatch");
    dispatch.bench_function("static_hit", |b| b.iter(|| mixed_static_hit(&router)));
    dispatch.bench_function("dynamic_fallback_hit", |b| {
        b.iter(|| mixed_dynamic_hit(&router));
    });
    dispatch.bench_function("complete_miss", |b| b.iter(|| mixed_complete_miss(&router)));
    dispatch.bench_function("static_capture_error", |b| {
        b.iter(|| mixed_static_capture_error(&router));
    });
    dispatch.finish();
}

criterion_group!(benches, mixed_routes);
criterion_main!(benches);
