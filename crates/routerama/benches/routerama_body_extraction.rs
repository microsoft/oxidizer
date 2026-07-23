// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation diagnostics for five response-equivalent bounded
//! request-body extraction fixtures. Rocket's split-body row is explicitly
//! named as a coalesced client body because its local client cannot retain
//! frame boundaries.
//!
//! Paired with `routerama_body_extraction_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports three harnesses")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/body_extraction_scenarios.rs");

fn body_extraction(c: &mut Criterion) {
    let fixtures = Fixtures::new_checked();
    let allocation_sweeps = fixtures.record_allocation_sweeps();
    let mut stderr = std::io::stderr().lock();
    for (framework, bytes) in Framework::ALL.into_iter().zip(allocation_sweeps) {
        writeln!(
            stderr,
            "allocation sweep (complete extraction/response observation; not handler-entry)/{framework:?}: {bytes} bytes"
        )
        .expect("writing benchmark diagnostics to stderr should succeed");
    }

    for scenario in Scenario::ALL {
        let mut group = c.benchmark_group(format!("routerama_body_extraction/{}", scenario.name()));
        for framework in Framework::ALL {
            let benchmark_name = if scenario == Scenario::BytesSplitSuccess && framework == Framework::Rocket {
                "rocket_coalesced_client_body"
            } else {
                framework.name()
            };
            group.bench_function(benchmark_name, |b| {
                b.iter_batched(
                    || fixtures.prepare(framework, scenario),
                    |call| std::hint::black_box(call()),
                    BatchSize::SmallInput,
                );
            });
        }
        group.finish();
    }
}

criterion_group!(benches, body_extraction);
criterion_main!(benches);
