// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation diagnostics for runtime-mounted services.
//!
//! `static_hit` measures what configuring erased mounts costs a generated
//! static request, `dynamic_dispatch` compares a configured generated dynamic
//! handler against an erased mounted service through the same entry, and
//! `standalone`, `captures`, `streaming`, `depth`, and `table_size` drive
//! `ErasedMountRouter::route` directly. Mounted results deliberately select a
//! type-erased capability boundary and must not be folded into the
//! generated-static five-framework comparisons.
//!
//! Paired with `routerama_mount_cg.rs`.

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

include!("common/mount_scenarios.rs");

fn print_diagnostics() {
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics() {
        let named = scenario.decomposition();
        writeln!(
            stderr,
            "mount allocations/{}: routing={} allocations/{} bytes, observing={} allocations/{} bytes, \
             named future={} body={} error={} scratch={}",
            scenario.diagnostic_name(),
            stats.routing.allocations,
            stats.routing.bytes,
            stats.observing.allocations,
            stats.observing.bytes,
            named.future,
            named.body,
            named.error,
            named.scratch,
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }
    for (scenario, calls) in mounted_call_counts() {
        writeln!(stderr, "mount erased-service calls/{}: {calls}", scenario.diagnostic_name())
            .expect("writing mounted-call diagnostics to stderr should succeed");
    }
}

fn mounts(c: &mut Criterion) {
    assert_equivalent();
    assert_allocation_decomposition();
    print_diagnostics();

    for subgroup in Scenario::SUBGROUPS {
        let mut group = c.benchmark_group(format!("routerama_mount/{subgroup}"));
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

criterion_group!(benches, mounts);
criterion_main!(benches);
