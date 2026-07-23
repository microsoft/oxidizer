// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the Tower transport adapter.
//!
//! Paired with `routerama_tower.rs`. The `SendBoxBody` case includes its
//! allocator call, because that allocation is the cost of the explicit erasure
//! boundary.

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

    include!("common/tower_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_prepared(prepared))
            }
        };
    }

    benchmark_case!(dispatch_direct_route, DirectRoute);
    benchmark_case!(dispatch_route_service_exact_body, RouteServiceExactBody);
    benchmark_case!(dispatch_route_service_send_box_body, RouteServiceSendBoxBody);

    library_benchmark_group!(
        name = dispatch;
        benchmarks =
            dispatch_direct_route,
            dispatch_route_service_exact_body,
            dispatch_route_service_send_box_body
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
