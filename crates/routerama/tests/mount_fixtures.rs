// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence, mounted-call, and allocation contracts for mount fixtures.

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

include!("../benches/common/mount_scenarios.rs");

#[test]
fn every_mount_scenario_produces_its_declared_response() {
    assert_equivalent();
}

#[test]
fn only_mounted_hits_invoke_an_erased_service() {
    for (scenario, calls) in mounted_call_counts() {
        let expected = usize::from(scenario.invokes_mounted_service());
        assert_eq!(
            calls,
            expected,
            "{} invoked {calls} mounted services; expected {expected}",
            scenario.diagnostic_name()
        );
    }
}

#[test]
fn mount_paths_allocate_only_what_the_decomposition_names() {
    assert_allocation_decomposition();
}

/// The named categories are only trustworthy if each one is pinned by a pair
/// that varies exactly one thing. These are those pairs, asserted directly on
/// measured counts rather than on the declared decomposition.
#[test]
fn each_named_allocation_is_isolated_by_a_differential_pair() {
    let measured: Vec<(Scenario, PhasedStats)> = allocation_diagnostics();
    let stats = |wanted: Scenario| {
        measured
            .iter()
            .find(|(scenario, _)| *scenario == wanted)
            .map(|(_, stats)| *stats)
            .expect("every scenario is measured")
    };

    // A generated static request over a populated mount table allocates
    // nothing, so every count below belongs to the erased boundary itself.
    assert_eq!(stats(Scenario::StaticWithPopulatedMounts).allocations(), 0);

    // A complete mount-table miss invokes no service: its single allocation is
    // the boxed fixed 404 body, which pins the `body` category at one.
    assert_eq!(stats(Scenario::StandaloneMiss).routing.allocations, 1);

    // A mounted literal hit adds exactly the boxed erased-service future.
    assert_eq!(
        stats(Scenario::StandaloneLiteral).routing.allocations,
        stats(Scenario::StandaloneMiss).routing.allocations + 1,
        "a mounted hit must add exactly one boxed erased-service future"
    );

    // Crossing the four-capture inline boundary adds exactly one capture
    // scratch allocation.
    assert_eq!(
        stats(Scenario::Captures(CaptureCount::Four)).routing.allocations,
        stats(Scenario::Captures(CaptureCount::None)).routing.allocations,
        "up to four captures must stay inline"
    );
    assert_eq!(
        stats(Scenario::Captures(CaptureCount::Five)).routing.allocations,
        stats(Scenario::Captures(CaptureCount::Four)).routing.allocations + 1,
        "the fifth capture must spill exactly one scratch allocation"
    );

    // Crossing the sixteen-segment inline boundary adds exactly one matcher
    // offset scratch allocation.
    assert_eq!(
        stats(Scenario::Depth(true)).routing.allocations,
        stats(Scenario::Depth(false)).routing.allocations + 1,
        "the seventeenth segment must spill exactly one scratch allocation"
    );

    // A failing body adds exactly one boxed error, and only while observing.
    assert_eq!(
        stats(Scenario::StreamingSuccess).observing.allocations,
        0,
        "a streaming mounted body that completes must not allocate while observed"
    );
    assert_eq!(
        stats(Scenario::StreamingError).observing.allocations,
        1,
        "a failing mounted body must allocate exactly one boxed error"
    );
    assert_eq!(
        stats(Scenario::StreamingError).routing.allocations,
        stats(Scenario::StreamingSuccess).routing.allocations,
        "a failing body must not change what routing allocates"
    );
}

/// Larger mount tables must not change what a mounted hit or miss allocates.
#[test]
fn mount_table_size_does_not_change_allocation_counts() {
    for (scenario, stats) in allocation_diagnostics() {
        let Scenario::Table(size, position) = scenario else {
            continue;
        };
        let expected = u64::from(!matches!(position, Position::Miss)) + 1;
        assert_eq!(
            stats.allocations(),
            expected,
            "a {}-entry mount table changed what {} allocates ({} bytes)",
            size.entries(),
            scenario.diagnostic_name(),
            stats.bytes()
        );
    }
}
