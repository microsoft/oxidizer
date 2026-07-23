// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation contracts for route-policy benchmark fixtures.

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

include!("../benches/common/route_policy_scenarios.rs");

#[test]
fn every_route_policy_scenario_produces_its_documented_response() {
    assert_equivalent();
}

#[test]
fn route_policy_allocates_only_for_a_negotiated_response_header() {
    for (scenario, stats) in allocation_diagnostics() {
        let expected = match scenario {
            // Overlap ranking, predicate rejection, the default miss, the typed
            // fallback, and the typed catcher all reach their response without
            // allocating. Only writing the negotiated `Content-Type` allocates,
            // and that is `http::HeaderMap`'s own first-insert storage.
            Scenario::PriorityHighestCandidate | Scenario::PriorityLowerCandidate | Scenario::PredicateAccepted => 2,
            // An uncaught request-parts rejection in a state-generic router
            // cannot name its rejection body, so the generated entry erases it
            // once. That single boxing keeps private rejection types out of the
            // public route signature and never touches a success path; a typed
            // catcher names its rejection and still allocates nothing.
            Scenario::CatcherDefaultRejection => 1,
            Scenario::PriorityPlain
            | Scenario::PredicateUnconstrained
            | Scenario::PredicateUnsupportedMediaType
            | Scenario::PredicateNotAcceptable
            | Scenario::FallbackDefaultMiss
            | Scenario::FallbackTypedMiss
            | Scenario::CatcherTypedRejection => 0,
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
