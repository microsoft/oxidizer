// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation diagnostics for Routerama route policy: overlap
//! priority, request predicates, typed routing fallback, and typed extractor
//! catchers. Each subgroup contains the plainest generated control that reaches
//! the same response boundary, so the reported cost is the policy's own.
//!
//! Paired with `routerama_route_policy_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![expect(
    clippy::panic,
    reason = "a pending or malformed in-memory fixture is a benchmark invariant violation"
)]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/route_policy_scenarios.rs");

fn print_diagnostics() {
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics() {
        writeln!(
            stderr,
            "route-policy allocations/{}: measured={} allocations/{} bytes",
            scenario.diagnostic_name(),
            stats.allocations,
            stats.bytes,
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }
}

fn route_policy(c: &mut Criterion) {
    assert_equivalent();
    print_diagnostics();

    for subgroup in ["priority", "predicates", "fallback", "catcher"] {
        let mut group = c.benchmark_group(format!("routerama_route_policy/{subgroup}"));
        for scenario in Scenario::ALL.into_iter().filter(|scenario| scenario.group() == subgroup) {
            group.bench_function(scenario.name(), |b| {
                b.iter_batched(
                    || prepare(scenario),
                    |prepared| std::hint::black_box(run_prepared(prepared)),
                    BatchSize::SmallInput,
                );
            });
        }
        group.finish();
    }
}

criterion_group!(benches, route_policy);
criterion_main!(benches);
