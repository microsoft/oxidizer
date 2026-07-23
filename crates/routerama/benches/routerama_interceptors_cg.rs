// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for generated interceptors.
//!
//! Paired with `routerama_interceptors.rs`.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
#[expect(
    clippy::panic,
    reason = "a pending or malformed in-memory fixture is a benchmark invariant violation"
)]
mod linux {
    use gungraun::prelude::*;

    include!("common/interceptor_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_prepared(prepared))
            }
        };
    }

    benchmark_case!(before_none, BeforeNone);
    benchmark_case!(before_one, BeforeOne);
    benchmark_case!(before_four, BeforeFour);

    benchmark_case!(after_none, AfterNone);
    benchmark_case!(after_one, AfterOne);
    benchmark_case!(after_four, AfterFour);

    benchmark_case!(transform_none, TransformNone);
    benchmark_case!(transform_bounded, TransformBounded);
    benchmark_case!(transform_streaming, TransformStreaming);

    library_benchmark_group!(
        name = before;
        benchmarks = before_none, before_one, before_four
    );
    library_benchmark_group!(
        name = after;
        benchmarks = after_none, after_one, after_four
    );
    library_benchmark_group!(
        name = transform;
        benchmarks = transform_none, transform_bounded, transform_streaming
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{after, before, transform};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = before, after, transform
);
