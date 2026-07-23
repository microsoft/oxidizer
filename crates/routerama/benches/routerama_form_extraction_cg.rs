// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for five response-equivalent bounded form extraction
//! fixtures. Rocket's split-body row is explicitly named as a coalesced client
//! body because its local client cannot retain frame boundaries.
//!
//! Paired with `routerama_form_extraction.rs`.

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

    include!("common/form_extraction_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $framework:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(setup_prepared(Framework::$framework, Scenario::$scenario))]
            fn $name(call: PreparedCall) -> Observation {
                std::hint::black_box(call())
            }
        };
    }

    benchmark_case!(form_single_success_routerama, Routerama, SingleSuccess);
    benchmark_case!(form_single_success_axum, Axum, SingleSuccess);
    benchmark_case!(form_single_success_actix_web, ActixWeb, SingleSuccess);
    benchmark_case!(form_single_success_rocket, Rocket, SingleSuccess);
    benchmark_case!(form_single_success_warp, Warp, SingleSuccess);

    benchmark_case!(form_split_success_routerama, Routerama, SplitSuccess);
    benchmark_case!(form_split_success_axum, Axum, SplitSuccess);
    benchmark_case!(form_split_success_actix_web, ActixWeb, SplitSuccess);
    benchmark_case!(form_split_success_rocket_coalesced_client_body, Rocket, SplitSuccess);
    benchmark_case!(form_split_success_warp, Warp, SplitSuccess);

    benchmark_case!(form_64_success_routerama, Routerama, AtLimitSuccess);
    benchmark_case!(form_64_success_axum, Axum, AtLimitSuccess);
    benchmark_case!(form_64_success_actix_web, ActixWeb, AtLimitSuccess);
    benchmark_case!(form_64_success_rocket, Rocket, AtLimitSuccess);
    benchmark_case!(form_64_success_warp, Warp, AtLimitSuccess);

    benchmark_case!(form_percent_encoded_success_routerama, Routerama, PercentEncodedSuccess);
    benchmark_case!(form_percent_encoded_success_axum, Axum, PercentEncodedSuccess);
    benchmark_case!(form_percent_encoded_success_actix_web, ActixWeb, PercentEncodedSuccess);
    benchmark_case!(form_percent_encoded_success_rocket, Rocket, PercentEncodedSuccess);
    benchmark_case!(form_percent_encoded_success_warp, Warp, PercentEncodedSuccess);

    benchmark_case!(form_optional_absent_success_routerama, Routerama, OptionalAbsentSuccess);
    benchmark_case!(form_optional_absent_success_axum, Axum, OptionalAbsentSuccess);
    benchmark_case!(form_optional_absent_success_actix_web, ActixWeb, OptionalAbsentSuccess);
    benchmark_case!(form_optional_absent_success_rocket, Rocket, OptionalAbsentSuccess);
    benchmark_case!(form_optional_absent_success_warp, Warp, OptionalAbsentSuccess);

    benchmark_case!(form_encoded_65_rejected_routerama, Routerama, OverLimit);
    benchmark_case!(form_encoded_65_rejected_axum, Axum, OverLimit);
    benchmark_case!(form_encoded_65_rejected_actix_web, ActixWeb, OverLimit);
    benchmark_case!(form_encoded_65_rejected_rocket, Rocket, OverLimit);
    benchmark_case!(form_encoded_65_rejected_warp, Warp, OverLimit);

    benchmark_case!(form_invalid_number_routerama, Routerama, InvalidNumber);
    benchmark_case!(form_invalid_number_axum, Axum, InvalidNumber);
    benchmark_case!(form_invalid_number_actix_web, ActixWeb, InvalidNumber);
    benchmark_case!(form_invalid_number_rocket, Rocket, InvalidNumber);
    benchmark_case!(form_invalid_number_warp, Warp, InvalidNumber);

    benchmark_case!(form_missing_field_routerama, Routerama, MissingField);
    benchmark_case!(form_missing_field_axum, Axum, MissingField);
    benchmark_case!(form_missing_field_actix_web, ActixWeb, MissingField);
    benchmark_case!(form_missing_field_rocket, Rocket, MissingField);
    benchmark_case!(form_missing_field_warp, Warp, MissingField);

    benchmark_case!(unsupported_form_content_type_routerama, Routerama, UnsupportedContentType);
    benchmark_case!(unsupported_form_content_type_axum, Axum, UnsupportedContentType);
    benchmark_case!(unsupported_form_content_type_actix_web, ActixWeb, UnsupportedContentType);
    benchmark_case!(unsupported_form_content_type_rocket, Rocket, UnsupportedContentType);
    benchmark_case!(unsupported_form_content_type_warp, Warp, UnsupportedContentType);

    benchmark_case!(missing_form_content_type_routerama, Routerama, MissingContentType);
    benchmark_case!(missing_form_content_type_axum, Axum, MissingContentType);
    benchmark_case!(missing_form_content_type_actix_web, ActixWeb, MissingContentType);
    benchmark_case!(missing_form_content_type_rocket, Rocket, MissingContentType);
    benchmark_case!(missing_form_content_type_warp, Warp, MissingContentType);

    library_benchmark_group!(
        name = form_single_success;
        benchmarks =
            form_single_success_routerama,
            form_single_success_axum,
            form_single_success_actix_web,
            form_single_success_rocket,
            form_single_success_warp
    );
    library_benchmark_group!(
        name = form_split_success;
        benchmarks =
            form_split_success_routerama,
            form_split_success_axum,
            form_split_success_actix_web,
            form_split_success_rocket_coalesced_client_body,
            form_split_success_warp
    );
    library_benchmark_group!(
        name = form_64_success;
        benchmarks =
            form_64_success_routerama,
            form_64_success_axum,
            form_64_success_actix_web,
            form_64_success_rocket,
            form_64_success_warp
    );
    library_benchmark_group!(
        name = form_percent_encoded_success;
        benchmarks =
            form_percent_encoded_success_routerama,
            form_percent_encoded_success_axum,
            form_percent_encoded_success_actix_web,
            form_percent_encoded_success_rocket,
            form_percent_encoded_success_warp
    );
    library_benchmark_group!(
        name = form_optional_absent_success;
        benchmarks =
            form_optional_absent_success_routerama,
            form_optional_absent_success_axum,
            form_optional_absent_success_actix_web,
            form_optional_absent_success_rocket,
            form_optional_absent_success_warp
    );
    library_benchmark_group!(
        name = form_encoded_65_rejected;
        benchmarks =
            form_encoded_65_rejected_routerama,
            form_encoded_65_rejected_axum,
            form_encoded_65_rejected_actix_web,
            form_encoded_65_rejected_rocket,
            form_encoded_65_rejected_warp
    );
    library_benchmark_group!(
        name = form_invalid_number;
        benchmarks =
            form_invalid_number_routerama,
            form_invalid_number_axum,
            form_invalid_number_actix_web,
            form_invalid_number_rocket,
            form_invalid_number_warp
    );
    library_benchmark_group!(
        name = form_missing_field;
        benchmarks =
            form_missing_field_routerama,
            form_missing_field_axum,
            form_missing_field_actix_web,
            form_missing_field_rocket,
            form_missing_field_warp
    );
    library_benchmark_group!(
        name = unsupported_form_content_type;
        benchmarks =
            unsupported_form_content_type_routerama,
            unsupported_form_content_type_axum,
            unsupported_form_content_type_actix_web,
            unsupported_form_content_type_rocket,
            unsupported_form_content_type_warp
    );
    library_benchmark_group!(
        name = missing_form_content_type;
        benchmarks =
            missing_form_content_type_routerama,
            missing_form_content_type_axum,
            missing_form_content_type_actix_web,
            missing_form_content_type_rocket,
            missing_form_content_type_warp
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{
    form_64_success, form_encoded_65_rejected, form_invalid_number, form_missing_field, form_optional_absent_success,
    form_percent_encoded_success, form_single_success, form_split_success, missing_form_content_type, unsupported_form_content_type,
};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups =
        form_single_success,
        form_split_success,
        form_64_success,
        form_percent_encoded_success,
        form_optional_absent_success,
        form_encoded_65_rejected,
        form_invalid_number,
        form_missing_field,
        unsupported_form_content_type,
        missing_form_content_type
);
