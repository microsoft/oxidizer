// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind comparisons for Routerama UTF-8 body extractors.
//!
//! Paired with `routerama_text_body.rs`.

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

    include!("common/text_body_scenarios.rs");

    macro_rules! benchmark_case {
        ($text_name:ident, $utf8_name:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $text_name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_text_prepared(prepared))
            }

            #[library_benchmark]
            #[bench::run(prepare(Scenario::$scenario))]
            fn $utf8_name(prepared: PreparedScenario) -> Observation {
                std::hint::black_box(run_utf8_prepared(prepared))
            }
        };
    }

    benchmark_case!(extraction_text_empty, extraction_utf8_empty, Empty);
    benchmark_case!(extraction_text_single, extraction_utf8_single, Single);
    benchmark_case!(extraction_text_split, extraction_utf8_split, Split);
    benchmark_case!(extraction_text_exact_limit, extraction_utf8_exact_limit, ExactLimit);
    benchmark_case!(extraction_text_invalid_utf8, extraction_utf8_invalid_utf8, InvalidUtf8);
    benchmark_case!(extraction_text_overflow, extraction_utf8_overflow, Overflow);
    benchmark_case!(extraction_text_body_error, extraction_utf8_body_error, BodyError);

    library_benchmark_group!(
        name = extraction;
        benchmarks =
            extraction_text_empty,
            extraction_utf8_empty,
            extraction_text_single,
            extraction_utf8_single,
            extraction_text_split,
            extraction_utf8_split,
            extraction_text_exact_limit,
            extraction_utf8_exact_limit,
            extraction_text_invalid_utf8,
            extraction_utf8_invalid_utf8,
            extraction_text_overflow,
            extraction_utf8_overflow,
            extraction_text_body_error,
            extraction_utf8_body_error
    );
}

#[cfg(target_os = "linux")]
pub use linux::extraction;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = extraction);
