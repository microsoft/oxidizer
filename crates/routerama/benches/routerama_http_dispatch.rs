// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock and allocation benchmarks for five behaviorally equivalent HTTP
//! routing and dispatch fixtures.
//!
//! Paired with `routerama_http_dispatch_cg.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports three harnesses")]

use std::io::Write as _;

use alloc_tracker::Allocator;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("common/http_dispatch_scenarios.rs");

fn http_dispatch(c: &mut Criterion) {
    let fixtures = Fixtures::new_checked();
    let allocation_sweeps = fixtures.record_allocation_sweeps();
    let mut stderr = std::io::stderr().lock();
    for (framework, bytes) in Framework::ALL.into_iter().zip(allocation_sweeps) {
        writeln!(stderr, "allocation sweep/{framework:?}: {bytes} bytes").expect("writing benchmark diagnostics to stderr should succeed");
    }

    for scenario in Scenario::ALL {
        let mut group = c.benchmark_group(format!("routerama_http_dispatch/{}", scenario.name()));
        for framework in Framework::ALL {
            group.bench_function(framework.name(), |b| {
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

criterion_group!(benches, http_dispatch);
criterion_main!(benches);
