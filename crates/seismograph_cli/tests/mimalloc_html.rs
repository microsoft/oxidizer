// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public CLI rendering of backend-tagged allocator captures.
#![cfg(not(miri))]

#[path = "../src/mimalloc_view/fixture.rs"]
#[expect(clippy::unwrap_used, reason = "small deterministic fixtures must encode successfully")]
mod fixture;
#[path = "../src/native_view/fixture.rs"]
mod native_fixture;
mod support;

#[test]
fn mimalloc_container_renders_global_heap_and_class_evidence_without_snmalloc_metrics() {
    let payload = fixture::snapshot();
    let bytes = fixture::container(&[fixture::source(&payload)]);

    let html = support::render_capture(&bytes, "mimalloc-state");

    for expected in [
        "Mimalloc structure explorer",
        "Heap owners: 10 / 12",
        "PARTIAL / bounded, truncated",
        "Arena bitmap commitment: 4096 B (includes tracked arena metadata)",
        "Direct OS data committed: 1024 B (excludes metadata commitment)",
        "Total OS commitment: UNKNOWN",
        "OS reservation: 10240 B (arenas + direct OS, metadata already included)",
        "Arena metadata: 512 B (already included, do not add again)",
        "class 60: 2 INDEXED regular abandoned pages",
        "Heap #11",
        "age 10 ns at capture",
        "contributed this round",
        "Block size: 32 B",
        "Backing page size: 65536 B",
        "Queued pages: 4 (excludes abandoned pages)",
        "Observed pages: 2",
        "Reserved blocks: 128 (includes uninitialized slots)",
        "Initialized capacity blocks: 100",
        "Used blocks: 40 (includes uncollected pending remote frees)",
        "Allocation-ready free blocks: 20",
        "Locally deferred free blocks: 10",
        "Retired pages: 1",
        "Class 73",
        "variable / no fixed size",
        "Bitmap commit is NOT physical residency",
        "Global scans read independent atomics",
        "NOT application-live memory",
        "Retained allocation events",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    for absent in [
        "Native v4 structure explorer",
        "Live requested",
        "Large outstanding native ranges",
        "Incoming atomic queue",
    ] {
        assert!(!html.contains(absent), "unexpected snmalloc/event-derived metric {absent}");
    }
}

#[test]
fn mimalloc_container_renders_indexed_abandoned_zeroes_without_proving_unindexed_absence() {
    let mut payload = fixture::snapshot();
    let seismograph_rallocator::snapshot::BackendState::Mimalloc(state) = &mut payload.backend else {
        unreachable!();
    };
    state.global.abandoned_pages.fill(0);
    let bytes = fixture::container(&[fixture::source(&payload)]);

    let html = support::render_capture(&bytes, "mimalloc-indexed-abandoned-zeroes");

    for expected in [
        "INDEXED regular abandoned pages by class (bins 0..60",
        "All INDEXED regular abandoned-page counts (bins 0..60) are zero",
        "Unindexed abandoned pages: UNKNOWN (huge/singleton pages and some OS-backed pages)",
        "Abandoned counts cover INDEXED regular pages in bins 0..60 only.",
        "Zeros do not prove their absence.",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("All abandoned-index counts are zero"));
}

#[test]
fn unknown_busy_unavailable_and_exited_heaps_keep_their_distinct_evidence() {
    let bytes = fixture::container(&[fixture::source(&fixture::snapshot())]);

    let html = support::render_capture(&bytes, "mimalloc-owner-evidence");

    for expected in [
        "Heap #12 · active · UNOBSERVED",
        "Heap #13 · active · UNAVAILABLE",
        "Diagnostic storage allocation failed",
        "Heap #14 · active · BUSY",
        "Heap #15 · active · BUSY · older round",
        "Heap #16 · exited · published · EXITED / historical observation",
        "Heap #17 · active · published · older session",
        "Heap #18 · active · published · newer than capture",
        "Heap #19 · active · published · older round",
        "Heap #20 · exited · UNOBSERVED · EXITED / state unknown",
        "Pages, block capacity, used blocks and free lists are UNKNOWN, not zero",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    let unobserved = html.split("Heap #12 ·").nth(1).unwrap().split("</details>").next().unwrap();
    assert!(!unobserved.contains("Used blocks:"));
}

#[test]
fn both_schema_four_backends_remain_visible_and_source_names_are_escaped() {
    let native: seismograph_rallocator::snapshot::Snapshot = native_fixture::snapshot().into();
    let mut mimalloc = fixture::source(&fixture::snapshot());
    mimalloc.name = "<mimalloc>&\"source\"".into();
    let bytes = fixture::container(&[fixture::source(&native), mimalloc]);

    let html = support::render_capture(&bytes, "coexisting-allocator-backends");

    for expected in [
        "Native v4 structure explorer",
        "Mimalloc structure explorer",
        "Large outstanding native ranges",
        "Initialized capacity blocks: 100",
        "&lt;mimalloc&gt;&amp;&quot;source&quot;",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("<mimalloc>"));
}

#[test]
fn current_snmalloc_payload_selects_its_own_explorer() {
    let html = support::render_html(&native_fixture::snapshot(), "current-snmalloc");

    assert!(html.contains("Native v4 structure explorer") && html.contains("Last contributor recorder thread 42"));
    assert!(!html.contains("Mimalloc structure explorer"));
}

#[test]
fn raw_schema_four_mimalloc_payload_selects_the_mimalloc_explorer() {
    let source = fixture::source(&fixture::snapshot());

    let html = support::render_capture(&source.data, "raw-mimalloc-payload");

    assert!(html.contains("Mimalloc structure explorer") && html.contains("Initialized capacity blocks: 100"));
    assert!(!html.contains("Native v4 structure explorer"));
}

#[test]
fn empty_partial_mimalloc_inventory_does_not_invent_complete_zero_coverage() {
    let mut snapshot = fixture::snapshot();
    let seismograph_rallocator::snapshot::BackendState::Mimalloc(state) = &mut snapshot.backend else {
        unreachable!();
    };
    state.owners.clear();
    snapshot.header.publication_enabled = false;
    let bytes = fixture::container(&[fixture::source(&snapshot)]);

    let html = support::render_capture(&bytes, "mimalloc-omitted-owners");

    for expected in [
        "Heap owners: 0 / 12",
        "inventory PARTIAL / bounded, truncated",
        "No heap rows were captured",
        "publication disabled",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert!(!html.contains("Heap #11"));
}
