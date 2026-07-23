// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation benchmarks for mixed static and runtime routing.
//!
//! Paired with `routerama_mixed_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "resolved benchmark variants are consumed through black_box")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/mixed_scenarios.rs");

fn mixed_routes(c: &mut Criterion) {
    let router = build_mixed_scenario();
    assert_equivalent(&router);
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics(&router) {
        writeln!(
            stderr,
            "mixed allocations/dispatch/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.allocations,
            stats.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }
    let mut dispatch = c.benchmark_group("routerama_mixed/dispatch");
    for scenario in Scenario::ALL {
        dispatch.bench_function(scenario.name(), |b| {
            b.iter(|| std::hint::black_box(run_scenario(&router, scenario)));
        });
    }
    dispatch.finish();
}

criterion_group!(benches, mixed_routes);
criterion_main!(benches);
