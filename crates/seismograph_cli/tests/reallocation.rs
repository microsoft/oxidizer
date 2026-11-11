// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Saved resize evidence reaches the in-memory HTML pipeline.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind, Reallocation};
use seismograph::recorder::event::{Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events};
use seismograph::recorder::thread::{ThreadId, ThreadLog};
use seismograph::snapshot::{DecodedSnapshot, test_util};

#[expect(dead_code, reason = "This suite uses the container helper, not the legacy payload helper")]
mod support;

fn event(kind: EventKind, payload: EventPayload, sequence: u64) -> Event {
    Event {
        thread_id: ThreadId::new(1),
        sequence: EventSequence::new(sequence),
        timestamp: EventTimestamp::from_ticks(sequence),
        kind,
        payload,
        call_stack: Vec::new(),
    }
}

#[test]
fn html_preserves_one_lifetime_and_displays_moved_in_place_and_orphan_resizes() {
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
    let mut snapshot = DecodedSnapshot {
        events: Events {
            clock: EventClock::CURRENT,
            total_events: 4,
            threads: vec![ThreadLog {
                thread_id: ThreadId::new(1),
                total_events: 4,
                lost_events: 0,
                name: "producer".into(),
            }],
            events: vec![
                event(EventKind::Allocation, EventPayload::Allocation(allocation), 1),
                event(EventKind::Reallocation, EventPayload::Reallocation(resize), 2),
                event(
                    EventKind::Reallocation,
                    EventPayload::Reallocation(Reallocation {
                        old_size: 96,
                        new_size: 8192,
                        new_address: Address::new(0x2000),
                        ..resize
                    }),
                    3,
                ),
                event(
                    EventKind::Reallocation,
                    EventPayload::Reallocation(Reallocation {
                        allocation_id: AllocationId::new(0x3000),
                        old_address: Address::new(0x3000),
                        new_address: Address::new(0x4000),
                        ..resize
                    }),
                    4,
                ),
            ],
            ..Events::default()
        },
        ..DecodedSnapshot::default()
    };
    let capture = test_util::encode(&snapshot).unwrap();

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
    assert_eq!(html.matches("<td>orphan resize</td>").count(), 1);
    assert!(html.contains("0x3000 / 64 B → 0x4000 / 96 B (moved)</td><td>orphan resize</td>"));

    snapshot.events.threads[0].total_events += 1;
    snapshot.events.total_events += 1;
    snapshot.events.events.push(event(
        EventKind::Deallocation,
        EventPayload::Allocation(Allocation {
            allocation_id: AllocationId::new(0x2000),
            address: Address::new(0x2000),
            size: 8192,
            ..allocation
        }),
        5,
    ));
    let capture = test_util::encode(&snapshot).unwrap();
    let html = support::render_capture(capture.as_bytes(), "resize-freed");
    assert!(html.contains("No retained unmatched allocation stacks."));
    assert_eq!(html.matches("<td>free</td>").count(), 1);
    assert!(html.contains("1 allocations · 8.00 KiB</text>"));
    assert!(html.contains("1 frees · 8.00 KiB</text>"));
    assert!(html.contains("8.00 KiB · 1 frees</title>"));
}
