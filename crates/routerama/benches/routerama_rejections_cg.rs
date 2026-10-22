// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for end-to-end Routerama rejection responses.
//!
//! Paired with `routerama_rejections.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
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
mod linux {
    use gungraun::prelude::*;

    include!("common/rejection_scenarios.rs");

    macro_rules! dispatch_case {
        ($name:ident, $scenario:expr) => {
            #[library_benchmark]
            #[bench::run(prepare($scenario))]
            fn $name(prepared: PreparedScenario) -> StatusCode {
                run_prepared(prepared)
            }
        };
    }

    dispatch_case!(dispatch_success, Scenario::Success);
    dispatch_case!(dispatch_not_found, Scenario::NotFound);
    dispatch_case!(dispatch_method_not_allowed, Scenario::MethodNotAllowed);
    dispatch_case!(dispatch_payload_too_large, Scenario::PayloadTooLarge);
    dispatch_case!(dispatch_malformed_json, Scenario::MalformedJson);

    library_benchmark_group!(
        name = dispatch;
        benchmarks =
            dispatch_success,
            dispatch_not_found,
            dispatch_method_not_allowed,
            dispatch_payload_too_large,
            dispatch_malformed_json
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::dispatch;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = dispatch
);
