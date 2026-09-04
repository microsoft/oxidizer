// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compatibility tests for rendering snapshots from older schemas.
#![cfg(not(miri))]
#![expect(clippy::too_many_lines, reason = "Migration fixtures are intentionally explicit")]

use seismograph_rallocator::callers::{AddressLookup, Callers, Event, EventKind, HeapKind};
use seismograph_rallocator::snapshot::{Domain, Estimate, Region, SizeClass, Snapshot, Stats, Version};
use seismograph_rallocator::topology::{Segment, Slice, SliceKind, TopologyRegion};

mod support;

macro_rules! schema {
    ($base:expr, $($field:ident: $value:expr),+ $(,)?) => {{
        let mut value = $base;
        $(value.$field = $value;)+
        value
    }};
}

fn allocated_event(allocation_id: u64, address: u64, size: u64, call_stack: Vec<u64>) -> Event {
    schema!(
        Event::default(),
        thread_log_id: 1,
        event_thread_id: 1,
        sequence: allocation_id,
        allocation_id: allocation_id,
        kind: EventKind::Allocated,
        heap_id: 1,
        heap_kind: HeapKind::General,
        freed_after_heap_release: false,
        address: address,
        size: size,
        align: 8,
        call_stack: call_stack
    )
}

#[test]
fn address_lookups_render_symbols_and_source_locations() {
    let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
    snapshot.callers = Some(schema!(
        Callers::default(),
        events: vec![allocated_event(1, 0x1000, 64, vec![0x1234])]
    ));
    snapshot.addresses.push(schema!(
        AddressLookup::default(),
        address: 0x1234,
        symbol: Some("example::allocate".to_owned()),
        filename: Some("src/example.rs".to_owned()),
        line: Some(42),
        column: Some(7)
    ));

    let html = support::render_html(&snapshot, "address-lookup");

    assert!(html.contains("example::allocate (src/example.rs:42:7) [0x0000000000001234]"));
}

#[test]
fn formatting_is_visible_in_snapshot_html() {
    let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
    snapshot.stats = schema!(
        Stats::default(),
        live_bytes: 999,
        mapped_bytes: 1024,
        allocations: 1_234_567
    );
    snapshot.size_classes.push(schema!(
        SizeClass::default(),
        class_index: 1,
        block_bytes: 1_u64 << 40,
        live_allocations: schema!(Estimate::default(), value: 5, lower_bound: 3, upper_bound: 7),
        requested_bytes: schema!(Estimate::default(), value: 2048, lower_bound: 1024, upper_bound: 4096),
        usable_bytes: schema!(Estimate::default(), value: 2048, lower_bound: 1024, upper_bound: 4096)
    ));
    snapshot.callers = Some(schema!(
        Callers::default(),
        events: vec![
            allocated_event(1, 0x1000, 1, vec![0x1234]),
            allocated_event(2, 0x2000, 1, vec![0x2345]),
            allocated_event(3, 0x3000, 1, vec![0x3456]),
            allocated_event(4, 0x4000, 1, vec![0x4567]),
        ]
    ));
    snapshot.addresses.extend([
        schema!(
            AddressLookup::default(),
            address: 0x1234,
            symbol: Some("<&\">".to_owned())
        ),
        schema!(
            AddressLookup::default(),
            address: 0x2345,
            filename: Some("file.rs".to_owned())
        ),
        schema!(
            AddressLookup::default(),
            address: 0x3456,
            filename: Some("file.rs".to_owned()),
            line: Some(7)
        ),
    ]);

    let html = support::render_html(&snapshot, "formatting");

    for expected in [
        "999 B",
        "1.00 KiB",
        "1.00 TiB",
        "1,234,567",
        "5 (3–7)",
        "2.00 KiB (1.00 KiB–4.00 KiB)",
        "&lt;&amp;&quot;&gt; [0x0000000000001234]",
        "0x0000000000002345 (file.rs)",
        "0x0000000000003456 (file.rs:7)",
        "0x0000000000004567",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[test]
fn callers_render_live_and_empty_stack_summaries() {
    let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
    snapshot.callers = Some(schema!(
        Callers::default(),
        session_id: 9,
        total_events: 3,
        lost_events: 1,
        events: vec![
            allocated_event(1, 0x1000, 64, vec![0x1234]),
            allocated_event(2, 0x2000, 32, vec![0x5678]),
            schema!(
                allocated_event(2, 0x2000, 32, Vec::new()),
                event_thread_id: 2,
                sequence: 3,
                allocation_id: 2,
                kind: EventKind::Deallocated,
                address: 0x2000,
                size: 32,
                align: 8,
                call_stack: Vec::new()
            ),
            schema!(
                allocated_event(99, 0x3000, 1, Vec::new()),
                event_thread_id: 1,
                sequence: 4,
                allocation_id: 99,
                kind: EventKind::Deallocated,
                address: 0x3000,
                size: 1,
                align: 8,
                call_stack: Vec::new()
            ),
        ],
    ));

    let html = support::render_html(&snapshot, "callers-live");
    assert!(html.contains("64 B unmatched in 1 allocations"));
    assert!(html.contains("0x0000000000001234"));

    snapshot.callers = Some(Callers::default());
    let empty_html = support::render_html(&snapshot, "callers-empty");
    assert!(empty_html.contains("No retained allocation stacks."));
}

#[test]
fn legacy_empty_snapshot_renders_unavailable_sections() {
    let html = support::render_html(&Snapshot::new(Version::new(1, 0, 0)), "legacy-empty");

    for expected in [
        "Domain telemetry is unavailable",
        "Detailed slice topology is unavailable",
        "Caller tracking was not available",
        "No classified small segments.",
        "No medium spans.",
        "No bump chunks.",
        "No owner identities were published.",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[test]
fn legacy_regions_and_zero_totals_render_without_physical_metadata() {
    let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
    snapshot.regions.push(schema!(
        Region::default(),
        region_index: 3,
        reserved_bytes: 4096,
        used_slices: 2,
        free_slices: 6
    ));
    snapshot.domains.push(schema!(
        Domain::default(),
        domain_id: 4,
        region_count: 0
    ));
    snapshot.size_classes.push(schema!(
        SizeClass::default(),
        class_index: 9,
        usable_bytes: Estimate::default()
    ));

    let html = support::render_html(&snapshot, "legacy-regions");

    assert!(html.contains("<td>#3</td><td>4.00 KiB</td><td>2</td><td>6</td>"));
    assert!(html.contains("<td>#4</td><td>—"));
    assert!(html.contains("width:0.000%"));
    assert!(html.contains("0.0%</td>"));
}

#[test]
fn detailed_topology_renders_every_slice_state_and_owner_group() {
    let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
    snapshot.domains.push(schema!(
        Domain::default(),
        domain_id: 8,
        is_default: true,
        region_count: 1,
        small_slices: 1,
        medium_slices: 3,
        bump_slices: 2,
        unknown_slices: 1,
        region_indices: vec![2]
    ));
    snapshot.domains.push(schema!(
        Domain::default(),
        domain_id: 9,
        region_count: 1,
        region_indices: vec![3]
    ));
    snapshot.size_classes.push(schema!(
        SizeClass::default(),
        class_index: 1,
        block_bytes: 64,
        requested_bytes: schema!(Estimate::default(), value: 32, lower_bound: 32, upper_bound: 32),
        usable_bytes: schema!(Estimate::default(), value: 64, lower_bound: 64, upper_bound: 64)
    ));
    snapshot.topology.push(schema!(
        TopologyRegion::default(),
        region_index: 2,
        base_address: 0x1000,
        region_bytes: 9 * 64,
        slice_bytes: 64,
        used_bitmap: vec![0b1_0111_1111],
        slices: vec![
            schema!(
                Slice::default(),
                slice_index: 0,
                kind: SliceKind::Small,
                span_slices: 1,
                owner: 0,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: vec![schema!(
                    Segment::default(),
                    segment_index: 0,
                    class_index: 1,
                    context: false,
                    live_blocks: 0,
                    usable_blocks: 0,
                    utilization_tracked: true
                )]
            ),
            schema!(
                Slice::default(),
                slice_index: 1,
                kind: SliceKind::Small,
                span_slices: 1,
                owner: 0x11,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: vec![
                    schema!(
                        Segment::default(),
                        segment_index: 0,
                        class_index: 99,
                        context: true,
                        live_blocks: 1,
                        usable_blocks: 2,
                        utilization_tracked: true
                    ),
                    schema!(
                        Segment::default(),
                        segment_index: 1,
                        class_index: 1,
                        context: true,
                        utilization_tracked: false
                    ),
                    schema!(
                        Segment::default(),
                        segment_index: 2,
                        class_index: 1,
                        context: false,
                        utilization_tracked: false
                    ),
                ],
            ),
            schema!(
                Slice::default(),
                slice_index: 2,
                kind: SliceKind::Medium,
                span_slices: 2,
                owner: 0x22,
                requested_bytes: 65,
                usable_bytes: 128,
                segments: Vec::new()
            ),
            schema!(
                Slice::default(),
                slice_index: 3,
                kind: SliceKind::MediumContinuation,
                span_slices: 0,
                owner: 0x22,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: Vec::new()
            ),
            schema!(
                Slice::default(),
                slice_index: 4,
                kind: SliceKind::Medium,
                span_slices: 1,
                owner: 0,
                requested_bytes: 0,
                usable_bytes: 64,
                segments: Vec::new()
            ),
            schema!(
                Slice::default(),
                slice_index: 5,
                kind: SliceKind::Bump,
                span_slices: 1,
                owner: 0,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: Vec::new()
            ),
            schema!(
                Slice::default(),
                slice_index: 6,
                kind: SliceKind::Bump,
                span_slices: 1,
                owner: 0x33,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: Vec::new()
            ),
            schema!(
                Slice::default(),
                slice_index: 8,
                kind: SliceKind::Small,
                span_slices: 1,
                owner: 0,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: vec![schema!(
                    Segment::default(),
                    segment_index: 0,
                    class_index: 1,
                    context: false,
                    live_blocks: 1,
                    usable_blocks: 2,
                    utilization_tracked: true
                )]
            ),
        ],
    ));
    snapshot.topology.push(schema!(
        TopologyRegion::default(),
        region_index: 3,
        region_bytes: 1,
        slice_bytes: 1,
        used_bitmap: vec![0]
    ));
    snapshot.callers = Some(schema!(
        Callers::default(),
        events: vec![allocated_event(1, 0x1000, 8, vec![0x10]), allocated_event(2, 0x2000, 8, vec![0x20])]
    ));
    snapshot.addresses.push(schema!(
        AddressLookup::default(),
        address: 0x10,
        symbol: Some("first".to_owned())
    ));

    let html = support::render_html(&snapshot, "detailed-topology");

    for expected in [
        "Allocated / transitional",
        "Small slab slice",
        "Medium span start",
        "Medium span continuation",
        "Bump chunk",
        "utilization unavailable",
        "Unknown",
        "Live allocation",
        "Retained free span",
        "0x0000000000000011",
        "0x0000000000000022",
        "0x0000000000000033",
        "first [0x0000000000000010]",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[test]
fn topology_boundaries_and_kinds_are_observable_in_html() {
    let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
    snapshot.topology.extend([
        schema!(
            TopologyRegion::default(),
            region_index: 4,
            region_bytes: 4,
            slice_bytes: 1,
            used_bitmap: vec![0b0101],
            slices: vec![
                schema!(Slice::default(), slice_index: 0),
                schema!(Slice::default(), slice_index: 2),
            ]
        ),
        schema!(
            TopologyRegion::default(),
            region_index: 5,
            region_bytes: 65,
            slice_bytes: 1,
            used_bitmap: vec![0, 1],
            slices: vec![schema!(Slice::default(), slice_index: 64)]
        ),
        schema!(
            TopologyRegion::default(),
            region_index: 6,
            region_bytes: 1,
            slice_bytes: 1,
            used_bitmap: vec![0]
        ),
    ]);

    let html = support::render_html(&snapshot, "topology-boundaries");

    for expected in [
        "aria-label=\"Region 4 slice map\"",
        "viewBox=\"0 0 2 2\"",
        "<tr><td>Used</td><td>0</td><td>1</td><td>1 B</td></tr>",
        "<tr><td>Free</td><td>1</td><td>1</td><td>1 B</td></tr>",
        "aria-label=\"Region 5 slice map\"",
        "viewBox=\"0 0 9 8\"",
        "slice 64 · Allocated / transitional",
        "aria-label=\"Region 6 slice map\"",
        "viewBox=\"0 0 1 1\"",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}
