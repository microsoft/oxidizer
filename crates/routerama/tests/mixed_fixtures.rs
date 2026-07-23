// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation contracts for mixed-routing benchmark fixtures.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/mixed_scenarios.rs");

#[test]
fn mixed_routes_preserve_short_and_deep_hit_miss_semantics() {
    let router = build_mixed_scenario();
    assert_equivalent(&router);
}

#[test]
fn deep_scan_allocation_boundary_is_visible() {
    let router = build_mixed_scenario();
    let diagnostics = allocation_diagnostics(&router);
    assert_eq!(
        diagnostics.map(|(scenario, stats)| (scenario, stats.allocations)),
        [
            (Scenario::ShortStaticHit, 0),
            (Scenario::ShortDynamicHit, 0),
            (Scenario::ShortMiss, 0),
            (Scenario::Deep17StaticHit, 1),
            (Scenario::Deep17DynamicHit, 1),
            (Scenario::Deep17Miss, 1),
            (Scenario::Deep32StaticHit, 1),
            (Scenario::Deep32DynamicHit, 1),
            (Scenario::Deep32Miss, 1),
        ],
        "static and dynamic fallback should share one deep-path scan allocation"
    );
}
