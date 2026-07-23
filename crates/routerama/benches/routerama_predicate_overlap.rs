// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock baselines for generated route-predicate overlap groups.
//!
//! Paired with `routerama_predicate_overlap_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

include!("common/predicate_overlap_scenarios.rs");

fn predicate_overlap(c: &mut Criterion) {
    assert_equivalent();
    for size in GroupSize::ALL {
        let mut group = c.benchmark_group(format!("routerama_predicate_overlap/{}", size.name()));
        for scenario in Scenario::ALL {
            group.bench_function(scenario.name(), |b| {
                b.iter_batched(
                    || prepare(size, scenario),
                    |prepared| std::hint::black_box(run_prepared(prepared)),
                    BatchSize::SmallInput,
                );
            });
        }
        group.finish();
    }
}

criterion_group!(benches, predicate_overlap);
criterion_main!(benches);
