// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation comparisons for Routerama UTF-8 body extractors.
//!
//! Paired with `routerama_text_body_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/text_body_scenarios.rs");

fn text_body(c: &mut Criterion) {
    assert_equivalent();
    assert_utf8_api_retains_single_frame();
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics() {
        writeln!(
            stderr,
            "text-body allocations/extraction/text/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.text.allocations,
            stats.text.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
        writeln!(
            stderr,
            "text-body allocations/extraction/utf8/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.utf8.allocations,
            stats.utf8.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }

    let mut group = c.benchmark_group("routerama_text_body/extraction");
    for scenario in Scenario::ALL {
        group.bench_function(BenchmarkId::new("text", scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_text_prepared(prepared)),
                BatchSize::SmallInput,
            );
        });
        group.bench_function(BenchmarkId::new("utf8", scenario.name()), |b| {
            b.iter_batched(
                || prepare(scenario),
                |prepared| std::hint::black_box(run_utf8_prepared(prepared)),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, text_body);
criterion_main!(benches);
