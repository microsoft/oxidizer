// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for five behaviorally equivalent HTTP routing and
//! dispatch fixtures.
//!
//! Paired with `routerama_http_dispatch.rs`.

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

    include!("common/http_dispatch_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $framework:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(setup_prepared(Framework::$framework, Scenario::$scenario))]
            fn $name(call: PreparedCall) -> Observation {
                std::hint::black_box(call())
            }
        };
    }

    benchmark_case!(literal_first_routerama, Routerama, LiteralFirst);
    benchmark_case!(literal_first_axum, Axum, LiteralFirst);
    benchmark_case!(literal_first_actix_web, ActixWeb, LiteralFirst);
    benchmark_case!(literal_first_rocket, Rocket, LiteralFirst);
    benchmark_case!(literal_first_warp, Warp, LiteralFirst);

    benchmark_case!(literal_middle_routerama, Routerama, LiteralMiddle);
    benchmark_case!(literal_middle_axum, Axum, LiteralMiddle);
    benchmark_case!(literal_middle_actix_web, ActixWeb, LiteralMiddle);
    benchmark_case!(literal_middle_rocket, Rocket, LiteralMiddle);
    benchmark_case!(literal_middle_warp, Warp, LiteralMiddle);

    benchmark_case!(literal_last_routerama, Routerama, LiteralLast);
    benchmark_case!(literal_last_axum, Axum, LiteralLast);
    benchmark_case!(literal_last_actix_web, ActixWeb, LiteralLast);
    benchmark_case!(literal_last_rocket, Rocket, LiteralLast);
    benchmark_case!(literal_last_warp, Warp, LiteralLast);

    benchmark_case!(captures_routerama, Routerama, Captures);
    benchmark_case!(captures_axum, Axum, Captures);
    benchmark_case!(captures_actix_web, ActixWeb, Captures);
    benchmark_case!(captures_rocket, Rocket, Captures);
    benchmark_case!(captures_warp, Warp, Captures);

    benchmark_case!(method_header_query_routerama, Routerama, MethodHeaderQuery);
    benchmark_case!(method_header_query_axum, Axum, MethodHeaderQuery);
    benchmark_case!(method_header_query_actix_web, ActixWeb, MethodHeaderQuery);
    benchmark_case!(method_header_query_rocket, Rocket, MethodHeaderQuery);
    benchmark_case!(method_header_query_warp, Warp, MethodHeaderQuery);

    benchmark_case!(response_status_header_routerama, Routerama, ResponseStatusHeader);
    benchmark_case!(response_status_header_axum, Axum, ResponseStatusHeader);
    benchmark_case!(response_status_header_actix_web, ActixWeb, ResponseStatusHeader);
    benchmark_case!(response_status_header_rocket, Rocket, ResponseStatusHeader);
    benchmark_case!(response_status_header_warp, Warp, ResponseStatusHeader);

    benchmark_case!(complete_miss_routerama, Routerama, CompleteMiss);
    benchmark_case!(complete_miss_axum, Axum, CompleteMiss);
    benchmark_case!(complete_miss_actix_web, ActixWeb, CompleteMiss);
    benchmark_case!(complete_miss_rocket, Rocket, CompleteMiss);
    benchmark_case!(complete_miss_warp, Warp, CompleteMiss);

    benchmark_case!(capture_conversion_failure_routerama, Routerama, CaptureConversionFailure);
    benchmark_case!(capture_conversion_failure_axum, Axum, CaptureConversionFailure);
    benchmark_case!(capture_conversion_failure_actix_web, ActixWeb, CaptureConversionFailure);
    benchmark_case!(capture_conversion_failure_rocket, Rocket, CaptureConversionFailure);
    benchmark_case!(capture_conversion_failure_warp, Warp, CaptureConversionFailure);

    library_benchmark_group!(
        name = literal_first;
        benchmarks =
            literal_first_routerama,
            literal_first_axum,
            literal_first_actix_web,
            literal_first_rocket,
            literal_first_warp
    );
    library_benchmark_group!(
        name = literal_middle;
        benchmarks =
            literal_middle_routerama,
            literal_middle_axum,
            literal_middle_actix_web,
            literal_middle_rocket,
            literal_middle_warp
    );
    library_benchmark_group!(
        name = literal_last;
        benchmarks =
            literal_last_routerama,
            literal_last_axum,
            literal_last_actix_web,
            literal_last_rocket,
            literal_last_warp
    );
    library_benchmark_group!(
        name = captures;
        benchmarks =
            captures_routerama,
            captures_axum,
            captures_actix_web,
            captures_rocket,
            captures_warp
    );
    library_benchmark_group!(
        name = method_header_query;
        benchmarks =
            method_header_query_routerama,
            method_header_query_axum,
            method_header_query_actix_web,
            method_header_query_rocket,
            method_header_query_warp
    );
    library_benchmark_group!(
        name = response_status_header;
        benchmarks =
            response_status_header_routerama,
            response_status_header_axum,
            response_status_header_actix_web,
            response_status_header_rocket,
            response_status_header_warp
    );
    library_benchmark_group!(
        name = complete_miss;
        benchmarks =
            complete_miss_routerama,
            complete_miss_axum,
            complete_miss_actix_web,
            complete_miss_rocket,
            complete_miss_warp
    );
    library_benchmark_group!(
        name = capture_conversion_failure;
        benchmarks =
            capture_conversion_failure_routerama,
            capture_conversion_failure_axum,
            capture_conversion_failure_actix_web,
            capture_conversion_failure_rocket,
            capture_conversion_failure_warp
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{
    capture_conversion_failure, captures, complete_miss, literal_first, literal_last, literal_middle, method_header_query,
    response_status_header,
};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups =
        literal_first,
        literal_middle,
        literal_last,
        captures,
        method_header_query,
        response_status_header,
        complete_miss,
        capture_conversion_failure
);
