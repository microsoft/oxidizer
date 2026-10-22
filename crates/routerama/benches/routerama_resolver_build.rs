// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for `RawResolver::new` and `RawResolver::new_checked`
//! across route-table sizes and shapes.
//!
//! Paired with `routerama_resolver_build_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports two harnesses")]

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

include!("common/resolver_build_scenarios.rs");

fn resolver_build(c: &mut Criterion) {
    assert_equivalent();

    let mut new_group = c.benchmark_group("routerama_resolver_build/new");
    for scenario in Scenario::ALL {
        new_group.bench_function(BenchmarkId::from_parameter(scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_new_prepared(prepared)),
                BatchSize::LargeInput,
            );
        });
    }
    new_group.finish();

    let mut new_checked_group = c.benchmark_group("routerama_resolver_build/new_checked");
    for scenario in Scenario::ALL {
        new_checked_group.bench_function(BenchmarkId::from_parameter(scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_new_checked_prepared(prepared)),
                BatchSize::LargeInput,
            );
        });
    }
    new_checked_group.finish();
}

criterion_group!(benches, resolver_build);
criterion_main!(benches);
