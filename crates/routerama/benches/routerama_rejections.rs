// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for end-to-end Routerama rejection responses.
//!
//! Paired with `routerama_rejections_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

include!("common/rejection_scenarios.rs");

fn rejections(c: &mut Criterion) {
    assert_equivalent();

    let mut group = c.benchmark_group("routerama_rejections/dispatch");
    for scenario in Scenario::ALL {
        group.bench_function(BenchmarkId::from_parameter(scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_prepared(prepared)),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, rejections);
criterion_main!(benches);
