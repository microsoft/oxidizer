// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks and diagnostics for Routerama response-body
//! representations.
//!
//! Paired with `routerama_response_body_cg.rs`.

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

include!("common/response_body_scenarios.rs");

fn print_diagnostics() {
    let mut stderr = std::io::stderr().lock();
    for diagnostic in allocation_diagnostics() {
        writeln!(
            stderr,
            "response-body allocations/{name}: setup={setup_count} allocations/{setup_bytes} bytes; \
             measured={measured_count} allocations/{measured_bytes} bytes",
            name = diagnostic.scenario.diagnostic_name(),
            setup_count = diagnostic.setup.allocations,
            setup_bytes = diagnostic.setup.bytes,
            measured_count = diagnostic.measured.allocations,
            measured_bytes = diagnostic.measured.bytes,
        )
        .expect("writing allocation diagnostics to stderr should succeed");
    }
    for (name, diagnostic) in ["fallible_part_success", "fallible_part_rejection"]
        .into_iter()
        .zip(response_part_allocation_diagnostics())
    {
        writeln!(
            stderr,
            "response-body allocations/{name}: measured={} allocations/{} bytes",
            diagnostic.allocations, diagnostic.bytes,
        )
        .expect("writing response-part allocation diagnostics to stderr should succeed");
    }

    let sizes = size_diagnostics();
    writeln!(
        stderr,
        "response-body sizes (host-specific; {}-bit {}-{}): \
         Body={} concrete_stream={} EitherBody<Body, concrete_stream>={} BoxBody={} BoxBodyError={} \
         fixed_service_future={} fixed_service_response={} fixed_service_opaque_body={} \
         multiple_service_future={} multiple_service_response={} multiple_service_opaque_body={} \
         generated_body_error_sum={}",
        usize::BITS,
        std::env::consts::ARCH,
        std::env::consts::OS,
        sizes.body,
        sizes.concrete_stream,
        sizes.either_body,
        sizes.box_body,
        sizes.box_body_error,
        sizes.fixed_service_future,
        sizes.fixed_service_response,
        sizes.fixed_service_opaque_body,
        sizes.multiple_service_future,
        sizes.multiple_service_response,
        sizes.multiple_service_opaque_body,
        sizes.generated_body_error_sum,
    )
    .expect("writing size diagnostics to stderr should succeed");
}

fn response_body(c: &mut Criterion) {
    assert_equivalent();
    assert_transport_compatibility();
    print_diagnostics();

    for subgroup in ["direct_observation", "generated_route", "error_propagation"] {
        let mut group = c.benchmark_group(format!("routerama_response_body/{subgroup}"));
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

criterion_group!(benches, response_body);
criterion_main!(benches);
