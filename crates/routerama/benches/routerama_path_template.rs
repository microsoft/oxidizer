// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for `PathTemplate::parse` across template shapes.
//!
//! Paired with `routerama_path_template_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports two harnesses")]

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

include!("common/path_template_scenarios.rs");

fn path_template(c: &mut Criterion) {
    assert_equivalent();

    let mut group = c.benchmark_group("routerama_path_template/parse");
    for scenario in Scenario::ALL {
        group.bench_function(BenchmarkId::from_parameter(scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_parse_prepared(&prepared)),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, path_template);
criterion_main!(benches);
