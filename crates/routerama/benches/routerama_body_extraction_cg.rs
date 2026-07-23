// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for five response-equivalent bounded request-body
//! extraction fixtures. Rocket's split-body row is explicitly named as a
//! coalesced client body because its local client cannot retain frame
//! boundaries.
//!
//! Paired with `routerama_body_extraction.rs`.

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

    include!("common/body_extraction_scenarios.rs");

    macro_rules! benchmark_case {
        ($name:ident, $framework:ident, $scenario:ident) => {
            #[library_benchmark]
            #[bench::run(setup_prepared(Framework::$framework, Scenario::$scenario))]
            fn $name(call: PreparedCall) -> Observation {
                std::hint::black_box(call())
            }
        };
    }

    benchmark_case!(bytes_single_success_routerama, Routerama, BytesSingleSuccess);
    benchmark_case!(bytes_single_success_axum, Axum, BytesSingleSuccess);
    benchmark_case!(bytes_single_success_actix_web, ActixWeb, BytesSingleSuccess);
    benchmark_case!(bytes_single_success_rocket, Rocket, BytesSingleSuccess);
    benchmark_case!(bytes_single_success_warp, Warp, BytesSingleSuccess);

    benchmark_case!(bytes_split_success_routerama, Routerama, BytesSplitSuccess);
    benchmark_case!(bytes_split_success_axum, Axum, BytesSplitSuccess);
    benchmark_case!(bytes_split_success_actix_web, ActixWeb, BytesSplitSuccess);
    benchmark_case!(bytes_split_success_rocket_coalesced_client_body, Rocket, BytesSplitSuccess);
    benchmark_case!(bytes_split_success_warp, Warp, BytesSplitSuccess);

    benchmark_case!(bytes_64_success_routerama, Routerama, BytesAtLimitSuccess);
    benchmark_case!(bytes_64_success_axum, Axum, BytesAtLimitSuccess);
    benchmark_case!(bytes_64_success_actix_web, ActixWeb, BytesAtLimitSuccess);
    benchmark_case!(bytes_64_success_rocket, Rocket, BytesAtLimitSuccess);
    benchmark_case!(bytes_64_success_warp, Warp, BytesAtLimitSuccess);

    benchmark_case!(text_success_routerama, Routerama, TextSuccess);
    benchmark_case!(text_success_axum, Axum, TextSuccess);
    benchmark_case!(text_success_actix_web, ActixWeb, TextSuccess);
    benchmark_case!(text_success_rocket, Rocket, TextSuccess);
    benchmark_case!(text_success_warp, Warp, TextSuccess);

    benchmark_case!(json_success_routerama, Routerama, JsonSuccess);
    benchmark_case!(json_success_axum, Axum, JsonSuccess);
    benchmark_case!(json_success_actix_web, ActixWeb, JsonSuccess);
    benchmark_case!(json_success_rocket, Rocket, JsonSuccess);
    benchmark_case!(json_success_warp, Warp, JsonSuccess);

    benchmark_case!(bytes_65_rejected_routerama, Routerama, BytesOverLimit);
    benchmark_case!(bytes_65_rejected_axum, Axum, BytesOverLimit);
    benchmark_case!(bytes_65_rejected_actix_web, ActixWeb, BytesOverLimit);
    benchmark_case!(bytes_65_rejected_rocket, Rocket, BytesOverLimit);
    benchmark_case!(bytes_65_rejected_warp, Warp, BytesOverLimit);

    benchmark_case!(text_utf8_65_rejected_routerama, Routerama, TextOverLimit);
    benchmark_case!(text_utf8_65_rejected_axum, Axum, TextOverLimit);
    benchmark_case!(text_utf8_65_rejected_actix_web, ActixWeb, TextOverLimit);
    benchmark_case!(text_utf8_65_rejected_rocket, Rocket, TextOverLimit);
    benchmark_case!(text_utf8_65_rejected_warp, Warp, TextOverLimit);

    benchmark_case!(json_encoded_65_rejected_routerama, Routerama, JsonOverLimit);
    benchmark_case!(json_encoded_65_rejected_axum, Axum, JsonOverLimit);
    benchmark_case!(json_encoded_65_rejected_actix_web, ActixWeb, JsonOverLimit);
    benchmark_case!(json_encoded_65_rejected_rocket, Rocket, JsonOverLimit);
    benchmark_case!(json_encoded_65_rejected_warp, Warp, JsonOverLimit);

    benchmark_case!(invalid_utf8_routerama, Routerama, InvalidUtf8);
    benchmark_case!(invalid_utf8_axum, Axum, InvalidUtf8);
    benchmark_case!(invalid_utf8_actix_web, ActixWeb, InvalidUtf8);
    benchmark_case!(invalid_utf8_rocket, Rocket, InvalidUtf8);
    benchmark_case!(invalid_utf8_warp, Warp, InvalidUtf8);

    benchmark_case!(malformed_json_routerama, Routerama, MalformedJson);
    benchmark_case!(malformed_json_axum, Axum, MalformedJson);
    benchmark_case!(malformed_json_actix_web, ActixWeb, MalformedJson);
    benchmark_case!(malformed_json_rocket, Rocket, MalformedJson);
    benchmark_case!(malformed_json_warp, Warp, MalformedJson);

    benchmark_case!(unsupported_json_content_type_routerama, Routerama, UnsupportedJsonContentType);
    benchmark_case!(unsupported_json_content_type_axum, Axum, UnsupportedJsonContentType);
    benchmark_case!(unsupported_json_content_type_actix_web, ActixWeb, UnsupportedJsonContentType);
    benchmark_case!(unsupported_json_content_type_rocket, Rocket, UnsupportedJsonContentType);
    benchmark_case!(unsupported_json_content_type_warp, Warp, UnsupportedJsonContentType);

    benchmark_case!(missing_json_content_type_routerama, Routerama, MissingJsonContentType);
    benchmark_case!(missing_json_content_type_axum, Axum, MissingJsonContentType);
    benchmark_case!(missing_json_content_type_actix_web, ActixWeb, MissingJsonContentType);
    benchmark_case!(missing_json_content_type_rocket, Rocket, MissingJsonContentType);
    benchmark_case!(missing_json_content_type_warp, Warp, MissingJsonContentType);

    library_benchmark_group!(
        name = bytes_single_success;
        benchmarks =
            bytes_single_success_routerama,
            bytes_single_success_axum,
            bytes_single_success_actix_web,
            bytes_single_success_rocket,
            bytes_single_success_warp
    );
    library_benchmark_group!(
        name = bytes_split_success;
        benchmarks =
            bytes_split_success_routerama,
            bytes_split_success_axum,
            bytes_split_success_actix_web,
            bytes_split_success_rocket_coalesced_client_body,
            bytes_split_success_warp
    );
    library_benchmark_group!(
        name = bytes_64_success;
        benchmarks =
            bytes_64_success_routerama,
            bytes_64_success_axum,
            bytes_64_success_actix_web,
            bytes_64_success_rocket,
            bytes_64_success_warp
    );
    library_benchmark_group!(
        name = text_success;
        benchmarks =
            text_success_routerama,
            text_success_axum,
            text_success_actix_web,
            text_success_rocket,
            text_success_warp
    );
    library_benchmark_group!(
        name = json_success;
        benchmarks =
            json_success_routerama,
            json_success_axum,
            json_success_actix_web,
            json_success_rocket,
            json_success_warp
    );
    library_benchmark_group!(
        name = bytes_65_rejected;
        benchmarks =
            bytes_65_rejected_routerama,
            bytes_65_rejected_axum,
            bytes_65_rejected_actix_web,
            bytes_65_rejected_rocket,
            bytes_65_rejected_warp
    );
    library_benchmark_group!(
        name = text_utf8_65_rejected;
        benchmarks =
            text_utf8_65_rejected_routerama,
            text_utf8_65_rejected_axum,
            text_utf8_65_rejected_actix_web,
            text_utf8_65_rejected_rocket,
            text_utf8_65_rejected_warp
    );
    library_benchmark_group!(
        name = json_encoded_65_rejected;
        benchmarks =
            json_encoded_65_rejected_routerama,
            json_encoded_65_rejected_axum,
            json_encoded_65_rejected_actix_web,
            json_encoded_65_rejected_rocket,
            json_encoded_65_rejected_warp
    );
    library_benchmark_group!(
        name = invalid_utf8;
        benchmarks =
            invalid_utf8_routerama,
            invalid_utf8_axum,
            invalid_utf8_actix_web,
            invalid_utf8_rocket,
            invalid_utf8_warp
    );
    library_benchmark_group!(
        name = malformed_json;
        benchmarks =
            malformed_json_routerama,
            malformed_json_axum,
            malformed_json_actix_web,
            malformed_json_rocket,
            malformed_json_warp
    );
    library_benchmark_group!(
        name = unsupported_json_content_type;
        benchmarks =
            unsupported_json_content_type_routerama,
            unsupported_json_content_type_axum,
            unsupported_json_content_type_actix_web,
            unsupported_json_content_type_rocket,
            unsupported_json_content_type_warp
    );
    library_benchmark_group!(
        name = missing_json_content_type;
        benchmarks =
            missing_json_content_type_routerama,
            missing_json_content_type_axum,
            missing_json_content_type_actix_web,
            missing_json_content_type_rocket,
            missing_json_content_type_warp
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{
    bytes_64_success, bytes_65_rejected, bytes_single_success, bytes_split_success, invalid_utf8, json_encoded_65_rejected, json_success,
    malformed_json, missing_json_content_type, text_success, text_utf8_65_rejected, unsupported_json_content_type,
};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups =
        bytes_single_success,
        bytes_split_success,
        bytes_64_success,
        text_success,
        json_success,
        bytes_65_rejected,
        text_utf8_65_rejected,
        json_encoded_65_rejected,
        invalid_utf8,
        malformed_json,
        unsupported_json_content_type,
        missing_json_content_type
);
