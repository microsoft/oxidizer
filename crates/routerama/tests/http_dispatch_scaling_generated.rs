// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Freshness and determinism checks for the generated scaling fixture.

#![cfg(not(miri))]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

#[path = "support/http_dispatch_scaling_codegen.rs"]
mod codegen;

#[test]
fn generated_fixture_is_deterministic_and_current() {
    let generated = codegen::generated_source();
    assert!(
        generated == codegen::generated_source(),
        "the scaling fixture generator must be deterministic"
    );
    assert!(
        include_str!("../benches/generated/http_dispatch_scaling.rs") == generated,
        "the generated scaling fixture is stale; run \
         `cargo +nightly -Zscript crates/routerama/scripts/generate_http_dispatch_scaling.rs` \
         from the repository root"
    );
}
