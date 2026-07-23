// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation contracts for interceptor benchmark fixtures.

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

include!("../benches/common/interceptor_scenarios.rs");

#[test]
fn zero_one_and_four_interceptors_produce_identical_responses() {
    assert_equivalent();
}

#[test]
fn passive_interceptors_allocate_nothing_and_only_buffering_costs_a_body() {
    for (scenario, stats) in allocation_diagnostics() {
        // Passive interceptors call directly, while every transform retains
        // the single frame rather than allocating a replacement buffer.
        let expected = 0;
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
