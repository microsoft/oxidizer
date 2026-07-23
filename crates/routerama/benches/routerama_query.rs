// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/query_scenarios.rs");

fn query_codecs(c: &mut Criterion) {
    assert_output_shapes();
    let mut stderr = std::io::stderr().lock();
    for diagnostic in output_diagnostics() {
        writeln!(
            stderr,
            "query-output allocations/{name}: measured={allocations} allocations/{bytes} bytes; length={length}; capacity={capacity}",
            name = diagnostic.shape.name(),
            allocations = diagnostic.allocations,
            bytes = diagnostic.bytes,
            length = diagnostic.length,
            capacity = diagnostic.capacity
        )
        .expect("writing query-output diagnostics to stderr should succeed");
    }
    let reserved = reserved_output_diagnostic();
    writeln!(
        stderr,
        "query-output allocations/caller_reserved: measured={} allocations/{} bytes; length={}; capacity={}",
        reserved.allocations, reserved.bytes, reserved.length, reserved.capacity
    )
    .expect("writing reserved query-output diagnostics to stderr should succeed");

    let mut parsing = c.benchmark_group("routerama_query/parse_common");
    parsing.bench_function("routerama", |b| b.iter(direct_parse_common));
    parsing.bench_function("serde_urlencoded", |b| b.iter(serde_urlencoded_parse_common));
    parsing.bench_function("serde_html_form", |b| b.iter(serde_html_form_parse_common));
    parsing.finish();

    let mut escaped = c.benchmark_group("routerama_query/parse_escaped");
    escaped.bench_function("routerama", |b| b.iter(direct_parse_escaped));
    escaped.bench_function("serde_urlencoded", |b| b.iter(serde_urlencoded_parse_escaped));
    escaped.bench_function("serde_html_form", |b| b.iter(serde_html_form_parse_escaped));
    escaped.finish();

    let mut repeated = c.benchmark_group("routerama_query/parse_repeated");
    repeated.bench_function("routerama", |b| b.iter(direct_parse_repeated));
    repeated.bench_function("serde_html_form", |b| b.iter(serde_html_form_parse_repeated));
    repeated.finish();

    let mut long = c.benchmark_group("routerama_query/parse_long_ascii");
    long.bench_function("routerama", |b| b.iter(direct_parse_long));
    long.bench_function("serde_urlencoded", |b| b.iter(serde_urlencoded_parse_long));
    long.bench_function("serde_html_form", |b| b.iter(serde_html_form_parse_long));
    long.finish();

    let direct = direct_common_value();
    let serde = serde_common_value();
    let mut output = String::with_capacity(64);
    let mut production = c.benchmark_group("routerama_query/produce_common");
    production.bench_function("routerama_reserved", |b| {
        b.iter(|| direct_produce_common(&direct, &mut output));
    });
    production.bench_function("serde_html_form_reserved", |b| {
        b.iter(|| serde_html_form_produce_common_reserved(&serde, &mut output));
    });
    production.finish();

    let mut allocating = c.benchmark_group("routerama_query/produce_common_allocating");
    allocating.bench_function("routerama", |b| {
        b.iter(|| direct_produce_common_allocating(&direct));
    });
    allocating.bench_function("serde_urlencoded", |b| {
        b.iter(|| serde_urlencoded_produce_common(&serde));
    });
    allocating.bench_function("serde_html_form", |b| {
        b.iter(|| serde_html_form_produce_common(&serde));
    });
    allocating.finish();

    let prepared = OutputShape::ALL.map(prepare_output_shape);
    let mut shapes = c.benchmark_group("routerama_query/produce_shapes");
    for (shape, prepared) in OutputShape::ALL.into_iter().zip(prepared.iter()) {
        shapes.bench_function(shape.name(), |b| {
            b.iter(|| std::hint::black_box(run_output_shape(prepared)));
        });
    }
    shapes.finish();
}

criterion_group!(benches, query_codecs);
criterion_main!(benches);
