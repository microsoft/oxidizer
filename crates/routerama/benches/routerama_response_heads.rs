// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation comparisons for Routerama response-head
//! construction representations.
//!
//! Paired with `routerama_response_heads_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/response_head_scenarios.rs");

fn response_heads(c: &mut Criterion) {
    assert_equivalent();
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics() {
        writeln!(
            stderr,
            "response-heads allocations/construction/reserved/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.reserved.allocations,
            stats.reserved.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
        writeln!(
            stderr,
            "response-heads allocations/construction/static_plan/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.static_plan.allocations,
            stats.static_plan.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
        writeln!(
            stderr,
            "response-heads allocations/construction/generated/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.generated.allocations,
            stats.generated.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }

    let mut group = c.benchmark_group("routerama_response_heads/construction");
    for scenario in Scenario::ALL {
        group.bench_function(BenchmarkId::new("reserved", scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_reserved_prepared(prepared)),
                BatchSize::SmallInput,
            );
        });
        group.bench_function(BenchmarkId::new("static_plan", scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_static_plan_prepared(prepared)),
                BatchSize::SmallInput,
            );
        });
        group.bench_function(BenchmarkId::new("generated", scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_generated_prepared(prepared)),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, response_heads);
criterion_main!(benches);
