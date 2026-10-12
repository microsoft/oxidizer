// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end native structure explorer rendering.
#![cfg(not(miri))]

#[path = "../src/native_view/fixture.rs"]
mod fixture;
mod support;

#[test]
fn retained_allocation_events_render_actors_stacks_locations_and_escaped_symbols() {
    use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
    use seismograph::recorder::event::{Address, EventClass, Record};
    use seismograph::snapshot::{SnapshotOptions, Source};

    static NATIVE: Source = Source::new(
        seismograph_rallocator::source::ID,
        "allocation-html-native",
        seismograph_rallocator::source::SCHEMA_VERSION,
        capture_native,
    );
    static SYMBOLS: Source = Source::new(
        seismograph_runtime::snapshot::source::ID,
        "allocation-html-symbols",
        6,
        capture_allocation_symbol,
    );
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy::all(true),
        ..Default::default()
    });
    let allocation = Allocation {
        allocation_id: AllocationId::new(7),
        event_thread_id: EventThreadId::new(70),
        heap_id: HeapId::new(1),
        heap_kind: HeapKind::General,
        freed_after_heap_release: false,
        address: Address::new(0x1234),
        size: 17,
        alignment: 8,
    };
    seismograph::record(EventClass::Allocation, || Some(Record::allocation(allocation)));
    seismograph::record(EventClass::Allocation, || {
        Some(Record::deallocation(Allocation {
            event_thread_id: EventThreadId::new(90),
            ..allocation
        }))
    });
    seismograph::snapshot::register_source(&NATIVE);
    seismograph::snapshot::register_source(&SYMBOLS);
    let recording = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::recorder(seismograph::recorder::Configuration::default());
    let html = support::render_capture(recording.as_bytes(), "retained-allocation-stacks");
    for (operation, actor) in [("alloc", 70), ("free", 90)] {
        let row = html
            .split("<tr>")
            .find(|row| row.starts_with(&format!("<td>{operation}</td>")) && row.contains("0x1234 / 17 B"))
            .unwrap()
            .split("</tr>")
            .next()
            .unwrap();
        assert!(row.contains(&format!("#{actor}</td>")), "{row}");
        assert!(row.contains("<td>matched retained pair</td>"), "{row}");
        assert!(row.contains("Operation stack"), "{row}");
    }
    let allocation_row = html
        .split("<tr>")
        .find(|row| row.starts_with("<td>alloc</td>") && row.contains("0x1234 / 17 B"))
        .unwrap()
        .split("</tr>")
        .next()
        .unwrap();
    assert!(allocation_row.contains("alloc&lt;T&gt;&amp;"), "{allocation_row}");
    assert!(allocation_row.contains("allocation.rs:42:7"), "{allocation_row}");
    assert!(!allocation_row.contains("alloc<T>&"), "{allocation_row}");
}

#[expect(clippy::unwrap_used, reason = "the source callback encodes a known valid test fixture")]
fn capture_native(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let snapshot = fixture::snapshot();
    let mut data = seismograph::snapshot::SourceData::zeroed(seismograph_rallocator::encoded_len(&snapshot).unwrap())?;
    seismograph_rallocator::encode(&snapshot, data.as_mut_bytes()).unwrap();
    Ok(data)
}

#[expect(clippy::unwrap_used, reason = "the test records an allocation with a required captured stack")]
fn capture_allocation_symbol(
    context: seismograph::snapshot::SnapshotContext<'_>,
) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let address = context
        .events()
        .events
        .iter()
        .find(|event| event.kind == seismograph::recorder::event::EventKind::Allocation)
        .unwrap()
        .call_stack
        .first()
        .unwrap()
        .get();
    // Runtime schema 6: no runtimes and one lookup for the captured allocation stack.
    let mut bytes = b"SEISRUNT".to_vec();
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&6_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&address.to_le_bytes());
    bytes.extend_from_slice(&9_u32.to_le_bytes());
    bytes.extend_from_slice(&13_u32.to_le_bytes());
    bytes.extend_from_slice(&42_u32.to_le_bytes());
    bytes.extend_from_slice(&7_u32.to_le_bytes());
    bytes.extend_from_slice(b"alloc<T>&allocation.rs");
    let mut data = seismograph::snapshot::SourceData::zeroed(bytes.len())?;
    data.as_mut_bytes().copy_from_slice(&bytes);
    Ok(data)
}

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
