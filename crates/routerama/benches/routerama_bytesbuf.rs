// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock comparisons for Routerama `BytesView` integration.
//!
//! Paired with `routerama_bytesbuf_cg.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(
    clippy::needless_pass_by_value,
    clippy::panic,
    reason = "prepared values delimit measured ownership and pending in-memory operations violate fixture invariants"
)]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/bytesbuf_scenarios.rs");

fn bytesbuf(c: &mut Criterion) {
    assert_equivalent();
    write_allocation_diagnostics();

    let mut buf = c.benchmark_group("routerama_bytesbuf/buf");
    for count in SpanCount::ALL {
        let prepared = prepare_view(count);
        buf.bench_function(BenchmarkId::new("chunks_vectored", count.name()), |b| {
            b.iter(|| std::hint::black_box(observe(std::hint::black_box(&prepared.view))));
        });
    }
    buf.finish();

    let mut response = c.benchmark_group("routerama_bytesbuf/response");
    for count in SpanCount::ALL {
        for (name, operation) in [
            ("direct", run_direct as fn(PreparedView) -> Observation),
            ("to_bytes_control", run_conversion),
        ] {
            response.bench_function(BenchmarkId::new(name, count.name()), |b| {
                b.iter_batched(
                    || prepare_view(count),
                    |prepared| std::hint::black_box(operation(prepared)),
                    BatchSize::SmallInput,
                );
            });
        }
        for (name, operation) in [
            ("generated", run_generated as fn(PreparedRoute) -> Observation),
            ("exact_tower", run_exact_tower),
            ("boxed_tower", run_boxed_tower),
        ] {
            response.bench_function(BenchmarkId::new(name, count.name()), |b| {
                b.iter_batched(
                    || prepare_route(count),
                    |prepared| std::hint::black_box(operation(prepared)),
                    BatchSize::SmallInput,
                );
            });
        }
    }
    response.finish();

    let mut extraction = c.benchmark_group("routerama_bytesbuf/extraction");
    for count in SpanCount::ALL {
        extraction.bench_function(count.name(), |b| {
            b.iter_batched(
                || prepare_extraction(count),
                |prepared| std::hint::black_box(run_extraction(prepared)),
                BatchSize::SmallInput,
            );
        });
    }
    extraction.finish();

    c.bench_function("routerama_bytesbuf/template/json", |b| {
        b.iter_batched(
            prepare_template,
            |prepared| std::hint::black_box(run_template(prepared)),
            BatchSize::SmallInput,
        );
    });
}

fn write_allocation_diagnostics() {
    let mut stderr = std::io::stderr().lock();
    for count in SpanCount::ALL {
        for (name, operation) in [
            ("direct", run_direct as fn(PreparedView) -> Observation),
            ("to_bytes_control", run_conversion),
        ] {
            let (allocations, bytes) = measure_allocations(name, prepare_view(count), operation);
            writeln!(
                stderr,
                "bytesbuf allocations/response/{name}/{}: measured={} allocations/{} bytes",
                count.name(),
                allocations,
                bytes
            )
            .expect("writing bytesbuf allocation diagnostics should succeed");
        }
        for (name, operation) in [
            ("generated", run_generated as fn(PreparedRoute) -> Observation),
            ("boxed_tower", run_boxed_tower),
        ] {
            let (allocations, bytes) = measure_allocations(name, prepare_route(count), operation);
            writeln!(
                stderr,
                "bytesbuf allocations/response/{name}/{}: measured={} allocations/{} bytes",
                count.name(),
                allocations,
                bytes
            )
            .expect("writing bytesbuf allocation diagnostics should succeed");
        }
    }
}

fn measure_allocations<T>(name: &str, prepared: T, operation: fn(T) -> Observation) -> (u64, u64) {
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let measured = session.operation(name);
    {
        let _span = measured.measure_thread().iterations(1);
        std::hint::black_box(operation(prepared));
    }
    let report = session.to_report();
    let (_, measured) = report
        .operations()
        .find(|(operation_name, _)| *operation_name == name)
        .expect("the bytesbuf allocation operation is recorded");
    (measured.total_allocations_count(), measured.total_bytes_allocated())
}

criterion_group!(benches, bytesbuf);
criterion_main!(benches);
