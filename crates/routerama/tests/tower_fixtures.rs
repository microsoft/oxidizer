// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation contracts for Tower-adapter fixtures.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]
#![expect(
    clippy::panic,
    reason = "a pending or malformed in-memory fixture is a benchmark invariant violation"
)]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/tower_scenarios.rs");

#[test]
fn every_tower_scenario_produces_an_identical_response() {
    assert_equivalent();
}

#[test]
fn the_adapter_allocates_nothing_and_send_erasure_costs_exactly_one_body() {
    for (scenario, stats) in allocation_diagnostics() {
        let expected = match scenario {
            Scenario::DirectRoute | Scenario::RouteServiceExactBody | Scenario::GeneratedExactTower => 0,
            Scenario::RouteServiceSendBoxBody => 1,
        };
        assert_eq!(
            stats.allocations,
            expected,
            "{} allocated {} times ({} bytes); expected {expected}",
            scenario.diagnostic_name(),
            stats.allocations,
            stats.bytes
        );
    }
}

#[test]
fn generated_exact_tower_keeps_the_exact_future_response_and_body_layout() {
    let sizes = size_diagnostics();
    assert_eq!(sizes.generated_future, sizes.exact_future);
    assert_eq!(sizes.generated_response, sizes.exact_response);
    assert_eq!(sizes.generated_body, sizes.exact_body);
    assert!(sizes.generated_body > sizes.send_boxed_body);
}
