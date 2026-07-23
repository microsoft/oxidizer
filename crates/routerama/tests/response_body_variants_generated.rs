// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Freshness and determinism checks for response-body variant controls.

#![cfg(not(miri))]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

#[path = "support/response_body_variants_codegen.rs"]
mod codegen;

#[test]
fn generated_variant_fixtures_are_deterministic_and_current() {
    let generated = codegen::generated_fixtures();
    let repeated = codegen::generated_fixtures();
    let committed = [
        include_str!("../benches/routerama_response_body_variants_1.rs"),
        include_str!("../benches/routerama_response_body_variants_4.rs"),
        include_str!("../benches/routerama_response_body_variants_16.rs"),
    ];

    for ((fixture, repeated), committed) in generated.into_iter().zip(repeated).zip(committed) {
        assert_eq!(
            fixture.source, repeated.source,
            "the {}-variant fixture changed between runs",
            fixture.variant_count
        );
        assert_eq!(
            fixture.source, committed,
            "the {}-variant fixture is stale; run \
             `cargo +nightly -Zscript crates/routerama/scripts/generate_response_body_variants.rs` \
             from the repository root",
            fixture.variant_count
        );
    }
}
