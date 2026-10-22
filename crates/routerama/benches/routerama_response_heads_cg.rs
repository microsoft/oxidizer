// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind comparisons for Routerama response-head construction
//! representations.
//!
//! Paired with `routerama_response_heads.rs`.

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
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::prelude::*;

    include!("common/response_head_scenarios.rs");

    macro_rules! benchmark_case {
        ($reserved_name:ident, $static_plan_name:ident, $generated_name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $reserved_name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_reserved_prepared(prepared))
            }

            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $static_plan_name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_static_plan_prepared(prepared))
            }

            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $generated_name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_generated_prepared(prepared))
            }
        };
    }

    benchmark_case!(
        construction_reserved_headers_0,
        construction_static_plan_headers_0,
        construction_generated_headers_0,
        Headers0
    );
    benchmark_case!(
        construction_reserved_headers_1,
        construction_static_plan_headers_1,
        construction_generated_headers_1,
        Headers1
    );
    benchmark_case!(
        construction_reserved_headers_4,
        construction_static_plan_headers_4,
        construction_generated_headers_4,
        Headers4
    );
    benchmark_case!(
        construction_reserved_headers_16,
        construction_static_plan_headers_16,
        construction_generated_headers_16,
        Headers16
    );

    library_benchmark_group!(
        name = construction;
        benchmarks =
            construction_reserved_headers_0,
            construction_static_plan_headers_0,
            construction_generated_headers_0,
            construction_reserved_headers_1,
            construction_static_plan_headers_1,
            construction_generated_headers_1,
            construction_reserved_headers_4,
            construction_static_plan_headers_4,
            construction_generated_headers_4,
            construction_reserved_headers_16,
            construction_static_plan_headers_16,
            construction_generated_headers_16
    );
}

#[cfg(target_os = "linux")]
pub use linux::construction;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = construction);
