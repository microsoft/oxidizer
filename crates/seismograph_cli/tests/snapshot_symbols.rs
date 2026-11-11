// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Saved common symbols reach the in-memory HTML pipeline without a source.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events};
use seismograph::recorder::thread::{ThreadId, ThreadLog};
use seismograph::snapshot::{DecodedSnapshot, decode, test_util};

#[test]
fn html_uses_symbols_resolved_by_the_producing_process_without_a_runtime_source() {
    let capture = test_util::capture(
        DecodedSnapshot {
            events: Events {
                clock: EventClock::CURRENT,
                total_events: 1,
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(1),
                    total_events: 1,
                    lost_events: 0,
                    name: "producer".into(),
                }],
                events: vec![Event {
                    thread_id: ThreadId::new(1),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(1),
                    kind: EventKind::Allocation,
                    payload: EventPayload::Allocation(Allocation {
                        allocation_id: AllocationId::new(0x1000),
                        event_thread_id: EventThreadId::new(0),
                        heap_id: HeapId::new(0),
                        heap_kind: HeapKind::General,
                        freed_after_heap_release: false,
                        address: Address::new(0x1000),
                        size: 64,
                        alignment: 8,
                    }),
                    call_stack: vec![Address::new(0x8000)],
                }],
                ..Events::default()
            },
            ..DecodedSnapshot::default()
        },
        |symbol| {
            assert_eq!(symbol.address, Address::new(0x8000));
            symbol.name = Some("producer::allocate".into());
            symbol.filename = Some("producer.rs".into());
            symbol.line = Some(42);
            symbol.column = Some(7);
        },
    )
    .unwrap();
    assert_eq!(decode(capture.as_bytes()).unwrap().sources, []);

    let html = seismograph_cli::test_util::render_capture(capture.as_bytes()).unwrap();

    assert!(html.contains("producer::allocate"));
    assert!(html.contains("producer.rs:42:7"));
}
