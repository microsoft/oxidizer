// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence checks for every generated HTTP dispatch scaling fixture.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports three harnesses")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

include!("../benches/common/http_dispatch_scaling_scenarios.rs");

#[test]
fn every_size_framework_and_scenario_is_equivalent() {
    let _fixtures = Fixtures::new_checked();
}
