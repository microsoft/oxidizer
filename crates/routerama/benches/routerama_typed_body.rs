// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation comparisons for `JsonBody` and `FormBody`
//! construction.
//!
//! Paired with `routerama_typed_body_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/typed_body_scenarios.rs");

fn typed_body(c: &mut Criterion) {
    assert_equivalent();
    let mut stderr = std::io::stderr().lock();
    for (scenario, stats) in allocation_diagnostics() {
        writeln!(
            stderr,
            "typed-body allocations/construction/json/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.json.allocations,
            stats.json.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
        writeln!(
            stderr,
            "typed-body allocations/construction/form/{}: measured={} allocations/{} bytes",
            scenario.name(),
            stats.form.allocations,
            stats.form.bytes
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }

    let mut group = c.benchmark_group("routerama_typed_body/construction");
    group.bench_function(BenchmarkId::new("json", "flat"), |b| {
        b.iter_batched(
            prepare_flat,
            |prepared| std::hint::black_box(run_json_flat_prepared(prepared)),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new("form", "flat"), |b| {
        b.iter_batched(
            prepare_flat,
            |prepared| std::hint::black_box(run_form_flat_prepared(prepared)),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new("json", "escaped"), |b| {
        b.iter_batched(
            prepare_escaped,
            |prepared| std::hint::black_box(run_json_escaped_prepared(prepared)),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new("form", "escaped"), |b| {
        b.iter_batched(
            prepare_escaped,
            |prepared| std::hint::black_box(run_form_escaped_prepared(prepared)),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new("json", "large"), |b| {
        b.iter_batched(
            prepare_large,
            |prepared| std::hint::black_box(run_json_large_prepared(prepared)),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(BenchmarkId::new("form", "large"), |b| {
        b.iter_batched(
            prepare_large,
            |prepared| std::hint::black_box(run_form_large_prepared(prepared)),
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, typed_body);
criterion_main!(benches);
