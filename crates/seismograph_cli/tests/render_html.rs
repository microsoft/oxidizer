// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end native structure explorer rendering.
#![cfg(not(miri))]

#[path = "../src/native_view/fixture.rs"]
mod fixture;
mod support;

#[test]
fn native_inventory_is_wired_into_snapshot_html() {
    let html = support::render_html(&fixture::snapshot(), "native-explorer");
    for expected in [
        "Native v4 structure explorer",
        "contributed this round",
        "age 10 ns at capture",
        "not an exact current census",
        "pending/retained frees",
        "potential work only",
        "ready links NOT guaranteed",
        "stale lease",
        "stale session",
        "fresh idle inspection",
        "older round",
        "BUSY",
        "unknown / unobserved",
        "Owners: 7 / 9",
        "PARTIAL bounded walk",
        "object",
        "slab",
        "capacity",
        "Large outstanding native ranges",
        "Local metadata",
        "Outgoing returns",
        "Incoming atomic queue",
        "Last contributor recorder thread 42",
        "NOT application-live memory",
        "NOT pending bytes",
        "NOT guaranteed physically decommitted",
        "<details class=",
        "[####################]",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("Live requested"));
    assert!(!html.contains("Physical allocator topology"));
}

#[test]
fn zero_source_reports_incomplete_coverage_instead_of_fabricating_owners() {
    let html = support::render_html(&seismograph_rallocator::native::Snapshot::default(), "zero-source");
    assert!(html.contains("Owners: 0 / 0"));
    assert!(html.contains("PARTIAL bounded walk"));
    assert!(!html.contains("Last contributor recorder thread"));
}

#[test]
fn system_slot_allocation_failure_is_not_presented_as_never_observed() {
    use seismograph_rallocator::native::{ObservationSource, Owner, Snapshot};
    let snapshot = Snapshot {
        owner_count: 1,
        owners_complete: true,
        owners: vec![Owner {
            id: 42,
            generation: 1,
            leased: true,
            source: ObservationSource::Unavailable,
            observation: None,
        }],
        ..Snapshot::default()
    };
    let html = support::render_html(&snapshot, "slot-allocation-failure");
    for evidence in [
        "<details class=\"unavailable\">",
        "UNAVAILABLE",
        "System slot allocation failed",
        "unavailable 1",
        "not zero or merely never-observed",
        "accepted recorded allocation/free operations only",
        "sampled-out events",
    ] {
        assert!(html.contains(evidence), "missing {evidence}");
    }

    assert!(!html.contains("unknown / unobserved"));
}

#[test]
fn matching_round_retains_age_and_does_not_claim_a_current_census() {
    let mut snapshot = fixture::snapshot();
    snapshot.captured_nanos = 1_000_090;
    let html = support::render_html(&snapshot, "old-first-round");
    assert!(html.contains("age 1000000 ns at capture"));
    assert!(html.contains("contributed this round"));
    assert!(html.contains("first round may predate polling"));
    assert!(!html.contains("current published"));
}

#[test]
fn equal_incoming_endpoints_do_not_claim_an_empty_queue() {
    let mut snapshot = fixture::snapshot();
    let observation = snapshot.owners[0].observation.as_mut().unwrap();
    observation.remote.incoming_front = 0xabc;
    observation.remote.incoming_back = 0xabc;
    let html = support::render_html(&snapshot, "equal-incoming-endpoints");
    assert!(html.contains("Sampled front != back: false"));
    assert!(html.contains("equality does NOT prove emptiness"));
    assert!(html.contains("depth/emptiness UNKNOWN"));
}
