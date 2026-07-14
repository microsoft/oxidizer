// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock "compare routers" benchmark.
//!
//! Builds each router being compared from the shared route table (see
//! `common/routes_data.rs`) once, outside the measured region, then times a full
//! sweep of the shared request-path lookups through each. `routerama` is the
//! build-time generated `resolve`; the others are runtime routers. The matching
//! Callgrind instruction-count suite is `gungraun_routers.rs`.

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

    group.bench_function("routerama_static", |b| b.iter(routerama_static_lookups));

    #[cfg(feature = "dynamic")]
    {
        let dynamic = build_routerama_dynamic();
        group.bench_function("routerama_dynamic", |b| b.iter(|| routerama_dynamic_lookups(&dynamic)));
    }

    let matchit = build_matchit();
    group.bench_function("matchit", |b| b.iter(|| matchit_lookups(&matchit)));

    let path_tree = build_path_tree();
    group.bench_function("path_tree", |b| b.iter(|| path_tree_lookups(&path_tree)));

    let actix = build_actix();
    group.bench_function("actix_router", |b| b.iter(|| actix_lookups(&actix)));

    let regex_router = build_regex();
    group.bench_function("regex", |b| b.iter(|| regex_lookups(&regex_router)));

    let route_recognizer = build_route_recognizer();
    group.bench_function("route_recognizer", |b| b.iter(|| route_recognizer_lookups(&route_recognizer)));

    let routefinder = build_routefinder();
    group.bench_function("routefinder", |b| b.iter(|| routefinder_lookups(&routefinder)));

    group.finish();
}

criterion_group!(benches, compare_routers);
criterion_main!(benches);
