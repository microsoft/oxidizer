// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind evidence for response templates and Hyper HTTP/1 serialization.
//!
//! Paired with `routerama_response_templates.rs`.

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

    include!("common/response_template_scenarios.rs");

    macro_rules! body_case {
        ($name:ident, $representation:ident, $scenario:ident) => {
            #[library_benchmark]
            fn $name() -> BodyObservation {
                std::hint::black_box(run_body(Representation::$representation, BodyScenario::$scenario))
            }
        };
    }

    macro_rules! transport_case {
        ($name:ident, $representation:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(prepare_transport(Representation::$representation, BodyScenario::$scenario))]
            fn $name(prepared: (Representation, BodyScenario)) -> TransportObservation {
                std::hint::black_box(run_prepared_transport(prepared))
            }
        };
    }

    macro_rules! head_case {
        ($name:ident, $scenario:ident) => {
            #[library_benchmark]
            fn $name() -> HeadObservation {
                std::hint::black_box(run_head(HeadScenario::$scenario))
            }
        };
    }

    macro_rules! head_candidate_case {
        ($name:ident, $representation:ident, $scenario:ident, $negotiated:literal) => {
            #[library_benchmark]
            fn $name() -> HeadObservation {
                std::hint::black_box(run_head_with(
                    HeadRepresentation::$representation,
                    HeadScenario::$scenario,
                    $negotiated,
                ))
            }
        };
    }

    body_case!(in_memory_existing_contiguous_fully_static, ExistingContiguous, FullyStatic);
    body_case!(in_memory_existing_contiguous_numeric_json, ExistingContiguous, NumericJson);
    body_case!(in_memory_existing_contiguous_escaped_json, ExistingContiguous, EscapedJson);
    body_case!(in_memory_existing_contiguous_medium_text_shell, ExistingContiguous, MediumTextShell);
    body_case!(in_memory_exact_contiguous_fully_static, ExactContiguous, FullyStatic);
    body_case!(in_memory_exact_contiguous_numeric_json, ExactContiguous, NumericJson);
    body_case!(in_memory_exact_contiguous_escaped_json, ExactContiguous, EscapedJson);
    body_case!(in_memory_exact_contiguous_medium_text_shell, ExactContiguous, MediumTextShell);
    body_case!(in_memory_segmented_fully_static, Segmented, FullyStatic);
    body_case!(in_memory_segmented_numeric_json, Segmented, NumericJson);
    body_case!(in_memory_segmented_escaped_json, Segmented, EscapedJson);
    body_case!(in_memory_segmented_medium_text_shell, Segmented, MediumTextShell);
    transport_case!(hyper_http1_existing_contiguous_fully_static, ExistingContiguous, FullyStatic);
    transport_case!(hyper_http1_existing_contiguous_numeric_json, ExistingContiguous, NumericJson);
    transport_case!(hyper_http1_existing_contiguous_escaped_json, ExistingContiguous, EscapedJson);
    transport_case!(
        hyper_http1_existing_contiguous_medium_text_shell,
        ExistingContiguous,
        MediumTextShell
    );
    transport_case!(hyper_http1_exact_contiguous_fully_static, ExactContiguous, FullyStatic);
    transport_case!(hyper_http1_exact_contiguous_numeric_json, ExactContiguous, NumericJson);
    transport_case!(hyper_http1_exact_contiguous_escaped_json, ExactContiguous, EscapedJson);
    transport_case!(hyper_http1_exact_contiguous_medium_text_shell, ExactContiguous, MediumTextShell);
    transport_case!(hyper_http1_segmented_fully_static, Segmented, FullyStatic);
    transport_case!(hyper_http1_segmented_numeric_json, Segmented, NumericJson);
    transport_case!(hyper_http1_segmented_escaped_json, Segmented, EscapedJson);
    transport_case!(hyper_http1_segmented_medium_text_shell, Segmented, MediumTextShell);
    head_case!(response_head_headers_0, Headers0);
    head_case!(response_head_headers_1, Headers1);
    head_case!(response_head_headers_4, Headers4);
    head_case!(response_head_headers_16, Headers16);
    head_candidate_case!(response_head_reserved_headers_0, Reserved, Headers0, false);
    head_candidate_case!(response_head_reserved_headers_1, Reserved, Headers1, false);
    head_candidate_case!(response_head_reserved_headers_4, Reserved, Headers4, false);
    head_candidate_case!(response_head_reserved_headers_16, Reserved, Headers16, false);
    head_candidate_case!(response_head_static_plan_headers_0, StaticPlan, Headers0, false);
    head_candidate_case!(response_head_static_plan_headers_1, StaticPlan, Headers1, false);
    head_candidate_case!(response_head_static_plan_headers_4, StaticPlan, Headers4, false);
    head_candidate_case!(response_head_static_plan_headers_16, StaticPlan, Headers16, false);
    head_candidate_case!(response_head_generated_plan_headers_0, GeneratedPlan, Headers0, false);
    head_candidate_case!(response_head_generated_plan_headers_1, GeneratedPlan, Headers1, false);
    head_candidate_case!(response_head_generated_plan_headers_4, GeneratedPlan, Headers4, false);
    head_candidate_case!(response_head_generated_plan_headers_16, GeneratedPlan, Headers16, false);
    head_candidate_case!(response_head_ordinary_negotiated_headers_0, Ordinary, Headers0, true);
    head_candidate_case!(response_head_ordinary_negotiated_headers_1, Ordinary, Headers1, true);
    head_candidate_case!(response_head_ordinary_negotiated_headers_4, Ordinary, Headers4, true);
    head_candidate_case!(response_head_ordinary_negotiated_headers_16, Ordinary, Headers16, true);
    head_candidate_case!(response_head_reserved_negotiated_headers_0, Reserved, Headers0, true);
    head_candidate_case!(response_head_reserved_negotiated_headers_1, Reserved, Headers1, true);
    head_candidate_case!(response_head_reserved_negotiated_headers_4, Reserved, Headers4, true);
    head_candidate_case!(response_head_reserved_negotiated_headers_16, Reserved, Headers16, true);
    head_candidate_case!(response_head_static_plan_negotiated_headers_0, StaticPlan, Headers0, true);
    head_candidate_case!(response_head_static_plan_negotiated_headers_1, StaticPlan, Headers1, true);
    head_candidate_case!(response_head_static_plan_negotiated_headers_4, StaticPlan, Headers4, true);
    head_candidate_case!(response_head_static_plan_negotiated_headers_16, StaticPlan, Headers16, true);
    head_candidate_case!(response_head_generated_plan_negotiated_headers_0, GeneratedPlan, Headers0, true);
    head_candidate_case!(response_head_generated_plan_negotiated_headers_1, GeneratedPlan, Headers1, true);
    head_candidate_case!(response_head_generated_plan_negotiated_headers_4, GeneratedPlan, Headers4, true);
    head_candidate_case!(response_head_generated_plan_negotiated_headers_16, GeneratedPlan, Headers16, true);

    library_benchmark_group!(
        name = in_memory;
        benchmarks =
            in_memory_existing_contiguous_fully_static,
            in_memory_existing_contiguous_numeric_json,
            in_memory_existing_contiguous_escaped_json,
            in_memory_existing_contiguous_medium_text_shell,
            in_memory_exact_contiguous_fully_static,
            in_memory_exact_contiguous_numeric_json,
            in_memory_exact_contiguous_escaped_json,
            in_memory_exact_contiguous_medium_text_shell,
            in_memory_segmented_fully_static,
            in_memory_segmented_numeric_json,
            in_memory_segmented_escaped_json,
            in_memory_segmented_medium_text_shell
    );
    library_benchmark_group!(
        name = hyper_http1;
        benchmarks =
            hyper_http1_existing_contiguous_fully_static,
            hyper_http1_existing_contiguous_numeric_json,
            hyper_http1_existing_contiguous_escaped_json,
            hyper_http1_existing_contiguous_medium_text_shell,
            hyper_http1_exact_contiguous_fully_static,
            hyper_http1_exact_contiguous_numeric_json,
            hyper_http1_exact_contiguous_escaped_json,
            hyper_http1_exact_contiguous_medium_text_shell,
            hyper_http1_segmented_fully_static,
            hyper_http1_segmented_numeric_json,
            hyper_http1_segmented_escaped_json,
            hyper_http1_segmented_medium_text_shell
    );
    library_benchmark_group!(
        name = response_head;
        benchmarks =
            response_head_headers_0,
            response_head_headers_1,
            response_head_headers_4,
            response_head_headers_16,
            response_head_reserved_headers_0,
            response_head_reserved_headers_1,
            response_head_reserved_headers_4,
            response_head_reserved_headers_16,
            response_head_static_plan_headers_0,
            response_head_static_plan_headers_1,
            response_head_static_plan_headers_4,
            response_head_static_plan_headers_16,
            response_head_generated_plan_headers_0,
            response_head_generated_plan_headers_1,
            response_head_generated_plan_headers_4,
            response_head_generated_plan_headers_16,
            response_head_ordinary_negotiated_headers_0,
            response_head_ordinary_negotiated_headers_1,
            response_head_ordinary_negotiated_headers_4,
            response_head_ordinary_negotiated_headers_16,
            response_head_reserved_negotiated_headers_0,
            response_head_reserved_negotiated_headers_1,
            response_head_reserved_negotiated_headers_4,
            response_head_reserved_negotiated_headers_16,
            response_head_static_plan_negotiated_headers_0,
            response_head_static_plan_negotiated_headers_1,
            response_head_static_plan_negotiated_headers_4,
            response_head_static_plan_negotiated_headers_16,
            response_head_generated_plan_negotiated_headers_0,
            response_head_generated_plan_negotiated_headers_1,
            response_head_generated_plan_negotiated_headers_4,
            response_head_generated_plan_negotiated_headers_16
    );
}

#[cfg(target_os = "linux")]
pub use linux::{hyper_http1, in_memory, response_head};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = in_memory, hyper_http1, response_head);
