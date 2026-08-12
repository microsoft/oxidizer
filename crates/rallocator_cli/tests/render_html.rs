// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end tests for HTML report rendering.
#![cfg(not(miri))]
#![expect(clippy::too_many_lines, reason = "Large fixtures are intentionally assembled inline")]

use rallocator_telemetry::callers::{Callers, Event, EventKind, HeapKind, ThreadLog, ThreadName};
use rallocator_telemetry::snapshot::{Domain, Estimate, Histograms, Region, SizeClass, Snapshot, Stats, Version};
use rallocator_telemetry::topology::{Segment, Slice, SliceKind, TopologyRegion};

mod support;

macro_rules! schema {
    ($base:expr, $($field:ident: $value:expr),+ $(,)?) => {{
        let mut value = $base;
        $(value.$field = $value;)+
        value
    }};
}

#[test]
fn html_report_contains_required_sections() {
    let mut snapshot = Snapshot::new(Version::new(1, 2, 3));
    snapshot.stats = schema!(
        Stats::default(),
        live_bytes: 1024,
        mapped_bytes: 4096,
        allocations: 12,
        remote_frees: 2
    );
    snapshot.size_classes.push(schema!(
        SizeClass::default(),
        class_index: 1,
        block_bytes: 64,
        live_allocations: schema!(Estimate::default(), value: 2, lower_bound: 2, upper_bound: 2),
        requested_bytes: schema!(Estimate::default(), value: 100, lower_bound: 100, upper_bound: 100),
        usable_bytes: schema!(Estimate::default(), value: 128, lower_bound: 128, upper_bound: 128)
    ));
    snapshot.regions.push(schema!(
        Region::default(),
        region_index: 0,
        reserved_bytes: 1 << 30,
        used_slices: 10,
        free_slices: 90
    ));
    snapshot.domains.push(schema!(
        Domain::default(),
        domain_id: 7,
        is_default: true,
        region_count: 1,
        reserved_bytes: 1 << 30,
        used_slices: 2,
        free_slices: 16_382,
        small_slices: 1,
        medium_slices: 0,
        bump_slices: 1,
        unknown_slices: 0,
        region_indices: vec![0]
    ));
    snapshot.topology.push(schema!(
        TopologyRegion::default(),
        region_index: 0,
        base_address: 0x1000_0000,
        region_bytes: 64 * (64 << 10),
        slice_bytes: 64 << 10,
        used_bitmap: vec![0b11],
        slices: vec![
            schema!(
                Slice::default(),
                slice_index: 0,
                kind: SliceKind::Small,
                span_slices: 0,
                owner: 0x1234,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: vec![schema!(
                    Segment::default(),
                    segment_index: 0,
                    class_index: 1,
                    context: false,
                    live_blocks: 7,
                    usable_blocks: 511,
                    utilization_tracked: true
                )]
            ),
            schema!(
                Slice::default(),
                slice_index: 1,
                kind: SliceKind::Bump,
                span_slices: 1,
                owner: 0x5678,
                requested_bytes: 0,
                usable_bytes: 0,
                segments: Vec::new()
            ),
        ],
    ));
    snapshot.histograms = schema!(
        Histograms::default(),
        allocated: vec![0, 0, 4],
        live: vec![0, 0, 1]
    );
    snapshot.callers = Some(schema!(
        Callers::default(),
        session_id: 1,
        total_events: 4,
        lost_events: 0,
        threads: vec![schema!(
            ThreadLog::default(),
            thread_log_id: 10,
            total_events: 4,
            lost_events: 0,
            allocated_histogram: vec![0, 0, 1],
            live_histogram: vec![0, 0, 0]
        )],
        events: vec![
            schema!(
                Event::default(),
                thread_log_id: 10,
                event_thread_id: 10,
                sequence: 1,
                allocation_id: 20,
                kind: EventKind::Allocated,
                heap_id: 30,
                heap_kind: HeapKind::Bump,
                freed_after_heap_release: false,
                address: 0x2000,
                size: 4,
                align: 4,
                call_stack: vec![0x3000]
            ),
            schema!(
                Event::default(),
                thread_log_id: 10,
                event_thread_id: 11,
                sequence: 2,
                allocation_id: 20,
                kind: EventKind::Deallocated,
                heap_id: 30,
                heap_kind: HeapKind::Bump,
                freed_after_heap_release: true,
                address: 0x2000,
                size: 4,
                align: 4,
                call_stack: vec![0x4000]
            ),
            schema!(
                Event::default(),
                thread_log_id: 10,
                event_thread_id: 10,
                sequence: 3,
                allocation_id: 21,
                kind: EventKind::Allocated,
                heap_id: 31,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: 0x2100,
                size: 8,
                align: 8,
                call_stack: vec![0x3000]
            ),
            schema!(
                Event::default(),
                thread_log_id: 10,
                event_thread_id: 10,
                sequence: 4,
                allocation_id: 21,
                kind: EventKind::Deallocated,
                heap_id: 31,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: 0x2100,
                size: 8,
                align: 8,
                call_stack: vec![0x4000]
            ),
        ],
        thread_names: vec![
            schema!(
                ThreadName::default(),
                thread_id: 10,
                name: "producer".to_owned()
            ),
            schema!(
                ThreadName::default(),
                thread_id: 11,
                name: "consumer".to_owned()
            ),
        ],
    ));

    let html = support::render_html(&snapshot, "required-sections");
    for heading in [
        "Virtual regions",
        "Allocation domains",
        "Allocation size histograms",
        "Segments, spans, bumps, and owners",
        "General slabs and size classes",
        "Process totals",
        "Remote frees and reclamation",
        "Retained caller stacks",
    ] {
        assert!(html.contains(heading));
    }
    assert!(html.contains("producer 1.2.3"));
    assert!(html.contains("domain #7 default"));
    assert!(html.contains("name=\"topology-mode\" value=\"kind\" checked"));
    assert!(html.contains("name=\"topology-mode\" value=\"owner\""));
    assert!(html.contains("<section id=\"physical-topology\" class=\"topology-kind\""));
    assert!(html.contains("document.querySelector('#physical-topology')"));
    assert!(html.contains("Allocation sizes by thread"));
    assert!(html.contains("value=\"live\" checked"));
    assert!(html.contains("class=\"histogram histogram-live\""));
    assert!(html.contains("producer · #10"));
    assert!(html.contains("Cross-thread and escaped-lifetime hotspots"));
    assert!(html.contains("Allocation → free thread flow"));
    assert!(html.contains("producer · #10 → consumer · #11"));
    assert!(html.contains("producer · #10 → producer · #10"));
    assert!(html.contains("thread-flow-link local"));
    assert!(html.contains("class=\"thread-flow-endpoint source\""));
    assert!(html.contains("class=\"thread-flow-endpoint destination\""));
    assert!(html.contains("data-source=\"10\" data-destination=\"11\""));
    assert!(html.contains("flow.classList.toggle('has-selection'"));
    assert!(html.contains("item.dataset[selection.side] === selection.thread"));
    assert!(html.contains("M330 68 C500 68,700 68,870 68"));
    assert!(html.contains("viewBox=\"0 0 1200"));
    assert!(!html.contains("width:1600px"));
    assert!(html.contains("Allocated and freed on different threads"));
    assert!(html.contains("Freed after a bump heap handle was released"));
    assert!(html.contains("<details><summary>General live-allocation hotspots (showing"));
    assert!(html.contains("<details class=\"stack-details\"><summary>Stack trace"));
    let cross_thread = html.find("Cross-thread and escaped-lifetime hotspots").unwrap();
    let thread_sizes = html.find("<details><summary>Allocation sizes by thread</summary>").unwrap();
    assert!(cross_thread < thread_sizes);
    assert!(html.contains("<section><details><summary>Segments, spans, bumps, and owners"));
    assert!(html.contains("<section><details><summary>General slabs and size classes"));
    assert!(html.contains("class=\"domain-bar\""));
    assert!(html.contains("Payload efficiency"));
    assert!(html.contains("7 / 511 live (1.4%)"));
    assert!(html.contains("7 / 511 live blocks (1.4%)"));
    assert!(html.contains("fill-opacity:"));
    assert!(html.contains("Block size"));
    assert!(html.contains("<td>64 B</td>"));
    assert!(html.matches("class=\"info\"").count() > 20);
    assert!(html.contains("tabindex=\"0\""));
    assert!(html.contains("role=\"tooltip\""));
    assert!(html.contains("getBoundingClientRect"));
    assert!(html.contains("innerWidth - width - margin"));
    assert!(html.contains("position:fixed"));
    assert!(!html.contains("Size-class occupancy"));
    assert!(!html.contains("http://"));
    assert!(!html.contains("https://"));
}

#[test]
fn html_report_handles_empty_and_boundary_data() {
    let mut snapshot = Snapshot::new(Version::new(1, 2, 3));
    snapshot.histograms = schema!(
        Histograms::default(),
        allocated: vec![1],
        live: vec![0]
    );
    snapshot.topology = vec![schema!(
        TopologyRegion::default(),
        region_index: 1,
        region_bytes: (64 << 10) * 16_384,
        slice_bytes: 64 << 10,
        used_bitmap: vec![0; 256],
    )];
    snapshot.callers = Some(schema!(
        Callers::default(),
        threads: vec![schema!(
            ThreadLog::default(),
            thread_log_id: 77,
        )],
    ));

    let html = support::render_html(&snapshot, "empty-boundary-data");

    assert!(html.contains("viewBox=\"0 0 128 128\""));
    assert!(html.contains("<span class=\"histogram-label\">0 B</span>"));
    assert!(html.contains("Thread log #77"));
}
