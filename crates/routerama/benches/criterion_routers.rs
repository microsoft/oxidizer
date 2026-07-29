// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock "compare routers" benchmark.
//!
//! Each warmed router resolves the same route table and coerces captures to the
//! same types. `gungraun_routers.rs` provides matching instruction counts;
//! construction remains wall-clock-only.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(
    unreachable_pub,
    dead_code,
    reason = "the shared harness is `include!`d and its items are used selectively per bench"
)]

use criterion::{Criterion, criterion_group, criterion_main};

include!("common/harness.rs");

fn compare_routers(c: &mut Criterion) {
    let mut group = c.benchmark_group("compare_routers");

    let routerama_static = build_hot_routerama_static();
    group.bench_function("routerama_static", |b| b.iter(|| routerama_static_lookups(&routerama_static)));

    {
        let dynamic = build_hot_routerama_dynamic();
        group.bench_function("routerama_dynamic", |b| b.iter(|| routerama_dynamic_lookups(&dynamic)));
    }

    let matchit = build_hot_matchit();
    group.bench_function("matchit", |b| b.iter(|| matchit_lookups(&matchit)));

    let path_tree = build_hot_path_tree();
    group.bench_function("path_tree", |b| b.iter(|| path_tree_lookups(&path_tree)));

    let regex_router = build_hot_regex();
    group.bench_function("regex", |b| b.iter(|| regex_lookups(&regex_router)));

    let route_recognizer = build_hot_route_recognizer();
    group.bench_function("route_recognizer", |b| b.iter(|| route_recognizer_lookups(&route_recognizer)));

    group.finish();

    let mut misses = c.benchmark_group("compare_router_misses");
    misses.bench_function("routerama_static", |b| {
        b.iter(|| routerama_static_misses(&routerama_static));
    });
    {
        let dynamic = build_hot_routerama_dynamic();
        misses.bench_function("routerama_dynamic", |b| {
            b.iter(|| routerama_dynamic_misses(&dynamic));
        });
    }
    misses.bench_function("matchit", |b| b.iter(|| matchit_misses(&matchit)));
    misses.bench_function("path_tree", |b| b.iter(|| path_tree_misses(&path_tree)));
    misses.bench_function("regex", |b| b.iter(|| regex_misses(&regex_router)));
    misses.bench_function("route_recognizer", |b| {
        b.iter(|| route_recognizer_misses(&route_recognizer));
    });
    misses.finish();

    let mut construction = c.benchmark_group("router_construction");
    construction.sample_size(10);
    construction.bench_function("routerama_dynamic_46_routes", |b| {
        b.iter_with_large_drop(build_routerama_dynamic);
    });
    construction.bench_function("matchit_46_routes", |b| {
        b.iter_with_large_drop(build_matchit);
    });
    construction.bench_function("path_tree_46_routes", |b| {
        b.iter_with_large_drop(build_path_tree);
    });
    construction.bench_function("regex_46_routes", |b| b.iter_with_large_drop(build_regex));
    construction.bench_function("route_recognizer_46_routes", |b| {
        b.iter_with_large_drop(build_route_recognizer);
    });
    construction.finish();
}

criterion_group!(benches, compare_routers);
criterion_main!(benches);
