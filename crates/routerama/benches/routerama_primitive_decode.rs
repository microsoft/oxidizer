// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation baselines for percent-encoded primitives.
//!
//! Paired with `routerama_primitive_decode_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/primitive_decode_scenarios.rs");

fn primitive_decode(c: &mut Criterion) {
    assert_equivalent();
    let fixtures = prepare();
    let diagnostics = allocation_diagnostics();
    let mut stderr = std::io::stderr().lock();
    for (source_index, source) in Source::ALL.into_iter().enumerate() {
        for (scenario_index, scenario) in Scenario::ALL.into_iter().enumerate() {
            let stats = diagnostics[source_index][scenario_index];
            writeln!(
                stderr,
                "primitive-decode allocations/{}/{}: measured={} allocations/{} bytes",
                source.name(),
                scenario.name(),
                stats.allocations,
                stats.bytes
            )
            .expect("writing allocation diagnostics to stderr should succeed");
        }
    }

    for source in Source::ALL {
        let mut group = c.benchmark_group(format!("routerama_primitive_decode/{}", source.name()));
        for scenario in Scenario::ALL {
            group.bench_function(scenario.name(), |b| {
                b.iter(|| std::hint::black_box(run(&fixtures, source, scenario)));
            });
        }
        group.finish();
    }
}

criterion_group!(benches, primitive_decode);
criterion_main!(benches);
