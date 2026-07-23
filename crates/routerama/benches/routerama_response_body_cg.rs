// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for Routerama response-body representations.
//!
//! Paired with `routerama_response_body.rs`. The two explicit `BoxBody`
//! construction cases include allocator calls so their instruction counts
//! describe the complete opt-in boundary, not allocator latency.

#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
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

    include!("common/response_body_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $name(prepared: PreparedScenario) -> ScenarioObservation {
                std::hint::black_box(run_prepared(prepared))
            }
        };
    }

    benchmark_case!(direct_observation_fixed_body, DirectFixed);
    benchmark_case!(direct_observation_concrete_stream, DirectConcreteStream);
    benchmark_case!(direct_observation_box_body_wrap_and_observe, DirectBoxBody);

    benchmark_case!(generated_route_fixed_body, GeneratedFixed);
    benchmark_case!(generated_route_concrete_stream, GeneratedConcreteStream);
    benchmark_case!(generated_route_explicit_box_body, GeneratedBoxBody);

    benchmark_case!(error_propagation_generated_concrete, GeneratedConcreteError);
    benchmark_case!(error_propagation_boxed, BoxedError);

    library_benchmark_group!(
        name = direct_observation;
        benchmarks =
            direct_observation_fixed_body,
            direct_observation_concrete_stream,
            direct_observation_box_body_wrap_and_observe
    );
    library_benchmark_group!(
        name = generated_route;
        benchmarks =
            generated_route_fixed_body,
            generated_route_concrete_stream,
            generated_route_explicit_box_body
    );
    library_benchmark_group!(
        name = error_propagation;
        benchmarks = error_propagation_generated_concrete, error_propagation_boxed
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{direct_observation, error_propagation, generated_route};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = direct_observation, generated_route, error_propagation
);
