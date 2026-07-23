// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation diagnostics for the Tower transport adapter: the
//! generated `route` entry, `RouteService`'s default `ExactBody` boundary, and
//! the generated exact transport boundary and explicit `SendBoxBody` boundary.
//!
//! Paired with `routerama_tower_cg.rs`.

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

include!("common/tower_scenarios.rs");

fn print_diagnostics() {
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics() {
        writeln!(
            stderr,
            "tower allocations/{}: measured={} allocations/{} bytes",
            scenario.diagnostic_name(),
            stats.allocations,
            stats.bytes,
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }
    let sizes = size_diagnostics();
    writeln!(
        stderr,
        "tower sizes (host-specific; {}-bit {}-{}): \
         exact=future {}/response {}/body {}; \
         generated=future {}/response {}/body {}; \
         send_boxed=future {}/response {}/body {}",
        usize::BITS,
        std::env::consts::ARCH,
        std::env::consts::OS,
        sizes.exact_future,
        sizes.exact_response,
        sizes.exact_body,
        sizes.generated_future,
        sizes.generated_response,
        sizes.generated_body,
        sizes.send_boxed_future,
        sizes.send_boxed_response,
        sizes.send_boxed_body,
    )
    .expect("writing size diagnostics to stderr should succeed");
}

fn tower(c: &mut Criterion) {
    assert_equivalent();
    print_diagnostics();

    let mut group = c.benchmark_group("routerama_tower/dispatch");
    for scenario in Scenario::ALL {
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

criterion_group!(benches, tower);
criterion_main!(benches);
