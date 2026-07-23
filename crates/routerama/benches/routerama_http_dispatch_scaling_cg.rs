// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for equivalent generated route-set scaling fixtures.
//!
//! Paired with `routerama_http_dispatch_scaling.rs`.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "the shared fixture supports three harnesses")]
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
mod linux {
    use gungraun::prelude::*;

    include!("common/http_dispatch_scaling_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $size:ident, $framework:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(setup_prepared(RouteSetSize::$size, Framework::$framework, Scenario::$scenario))]
            fn $name(call: PreparedCall) -> Observation {
                std::hint::black_box(call())
            }
        };
    }

    macro_rules! scaling_group {
        (
            $group:ident,
            $size:ident,
            $scenario:ident,
            $routerama:ident,
            $axum:ident,
            $actix_web:ident,
            $rocket:ident,
            $warp:ident
        ) => {
            benchmark_case!($routerama, $size, Routerama, $scenario);
            benchmark_case!($axum, $size, Axum, $scenario);
            benchmark_case!($actix_web, $size, ActixWeb, $scenario);
            benchmark_case!($rocket, $size, Rocket, $scenario);
            benchmark_case!($warp, $size, Warp, $scenario);

            library_benchmark_group!(
                name = $group;
                benchmarks = $routerama, $axum, $actix_web, $rocket, $warp
            );
        };
    }

    scaling_group!(
        routes_16_first,
        Routes16,
        First,
        routes_16_first_routerama,
        routes_16_first_axum,
        routes_16_first_actix_web,
        routes_16_first_rocket,
        routes_16_first_warp
    );
    scaling_group!(
        routes_16_middle,
        Routes16,
        Middle,
        routes_16_middle_routerama,
        routes_16_middle_axum,
        routes_16_middle_actix_web,
        routes_16_middle_rocket,
        routes_16_middle_warp
    );
    scaling_group!(
        routes_16_last,
        Routes16,
        Last,
        routes_16_last_routerama,
        routes_16_last_axum,
        routes_16_last_actix_web,
        routes_16_last_rocket,
        routes_16_last_warp
    );
    scaling_group!(
        routes_16_miss,
        Routes16,
        Miss,
        routes_16_miss_routerama,
        routes_16_miss_axum,
        routes_16_miss_actix_web,
        routes_16_miss_rocket,
        routes_16_miss_warp
    );

    scaling_group!(
        routes_128_first,
        Routes128,
        First,
        routes_128_first_routerama,
        routes_128_first_axum,
        routes_128_first_actix_web,
        routes_128_first_rocket,
        routes_128_first_warp
    );
    scaling_group!(
        routes_128_middle,
        Routes128,
        Middle,
        routes_128_middle_routerama,
        routes_128_middle_axum,
        routes_128_middle_actix_web,
        routes_128_middle_rocket,
        routes_128_middle_warp
    );
    scaling_group!(
        routes_128_last,
        Routes128,
        Last,
        routes_128_last_routerama,
        routes_128_last_axum,
        routes_128_last_actix_web,
        routes_128_last_rocket,
        routes_128_last_warp
    );
    scaling_group!(
        routes_128_miss,
        Routes128,
        Miss,
        routes_128_miss_routerama,
        routes_128_miss_axum,
        routes_128_miss_actix_web,
        routes_128_miss_rocket,
        routes_128_miss_warp
    );

    scaling_group!(
        routes_1024_first,
        Routes1024,
        First,
        routes_1024_first_routerama,
        routes_1024_first_axum,
        routes_1024_first_actix_web,
        routes_1024_first_rocket,
        routes_1024_first_warp
    );
    scaling_group!(
        routes_1024_middle,
        Routes1024,
        Middle,
        routes_1024_middle_routerama,
        routes_1024_middle_axum,
        routes_1024_middle_actix_web,
        routes_1024_middle_rocket,
        routes_1024_middle_warp
    );
    scaling_group!(
        routes_1024_last,
        Routes1024,
        Last,
        routes_1024_last_routerama,
        routes_1024_last_axum,
        routes_1024_last_actix_web,
        routes_1024_last_rocket,
        routes_1024_last_warp
    );
    scaling_group!(
        routes_1024_miss,
        Routes1024,
        Miss,
        routes_1024_miss_routerama,
        routes_1024_miss_axum,
        routes_1024_miss_actix_web,
        routes_1024_miss_rocket,
        routes_1024_miss_warp
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{
    routes_16_first, routes_16_last, routes_16_middle, routes_16_miss, routes_128_first, routes_128_last, routes_128_middle,
    routes_128_miss, routes_1024_first, routes_1024_last, routes_1024_middle, routes_1024_miss,
};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups =
        routes_16_first,
        routes_16_middle,
        routes_16_last,
        routes_16_miss,
        routes_128_first,
        routes_128_middle,
        routes_128_last,
        routes_128_miss,
        routes_1024_first,
        routes_1024_middle,
        routes_1024_last,
        routes_1024_miss
);
