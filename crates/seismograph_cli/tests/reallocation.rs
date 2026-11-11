// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Saved resize evidence reaches the public HTML command.
#![cfg(not(miri))]

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind, Reallocation};
use seismograph::recorder::event::{Address, EventClass, Record};
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};

#[expect(dead_code, reason = "This suite uses the container helper, not the legacy payload helper")]
mod support;

#[test]
fn html_preserves_one_lifetime_and_displays_moved_in_place_and_orphan_resizes() {
    seismograph::recorder(Configuration {
        allocations: Policy::all(false),
        ..Configuration::default()
    });
    let allocation = Allocation {
        allocation_id: AllocationId::new(0x1000),
        event_thread_id: EventThreadId::new(0),
        heap_id: HeapId::new(0),
        heap_kind: HeapKind::General,
        freed_after_heap_release: false,
        address: Address::new(0x1000),
        size: 64,
        alignment: 8,
    };
    let resize = Reallocation {
        allocation_id: allocation.allocation_id,
        heap_id: allocation.heap_id,
        heap_kind: allocation.heap_kind,
        old_address: allocation.address,
        new_address: allocation.address,
        old_size: 64,
        new_size: 96,
        alignment: 8,
    };
    seismograph::record(EventClass::Allocation, || Some(Record::allocation(allocation)));
    seismograph::record(EventClass::Allocation, || Some(Record::reallocation(resize)));
    seismograph::record(EventClass::Allocation, || {
        Some(Record::reallocation(Reallocation {
            old_size: 96,
            new_size: 8192,
            new_address: Address::new(0x2000),
            ..resize
        }))
    });
    seismograph::record(EventClass::Allocation, || {
        Some(Record::reallocation(Reallocation {
            allocation_id: AllocationId::new(0x3000),
            old_address: Address::new(0x3000),
            new_address: Address::new(0x4000),
            ..resize
        }))
    });
    let capture = seismograph::snapshot(SnapshotOptions::default()).unwrap();

    let html = support::render_capture(capture.as_bytes(), "resize-unmatched");

    for expected in [
        "Allocation/resize/free records",
        "0x1000 / 64 B → 0x1000 / 96 B (in place)",
        "0x1000 / 96 B → 0x2000 / 8192 B (moved)",
        "orphan resize",
        "8.00 KiB unmatched in 1 allocations",
        "64 B allocated in 1, 0 freed",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
    assert_eq!(html.matches("<td>alloc</td>").count(), 1);
    assert_eq!(html.matches("<td>resize</td>").count(), 3);

    seismograph::record(EventClass::Allocation, || {
        Some(Record::deallocation(Allocation {
            allocation_id: AllocationId::new(0x2000),
            address: Address::new(0x2000),
            size: 8192,
            ..allocation
        }))
    });
    let capture = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    let html = support::render_capture(capture.as_bytes(), "resize-freed");
    assert!(html.contains("No retained unmatched allocation stacks."));
    assert_eq!(html.matches("<td>free</td>").count(), 1);
}
