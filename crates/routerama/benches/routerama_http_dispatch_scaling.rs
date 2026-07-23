// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for equivalent generated route-set scaling fixtures.
//!
//! Paired with `routerama_http_dispatch_scaling_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports three harnesses")]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

include!("common/http_dispatch_scaling_scenarios.rs");

fn http_dispatch_scaling(c: &mut Criterion) {
    let fixtures = Fixtures::new_checked();

    for size in RouteSetSize::ALL {
        for scenario in Scenario::ALL {
            let mut group = c.benchmark_group(format!("routerama_http_dispatch_scaling/{}_{}", size.name(), scenario.name()));
            for framework in Framework::ALL {
                group.bench_function(framework.name(), |b| {
                    b.iter_batched(
                        || fixtures.prepare(size, framework, scenario),
                        |call| std::hint::black_box(call()),
                        BatchSize::SmallInput,
                    );
                });
            }
            group.finish();
        }
    }
}

criterion_group!(benches, http_dispatch_scaling);
criterion_main!(benches);
