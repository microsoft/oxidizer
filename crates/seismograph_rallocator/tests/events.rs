// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authoritative container allocation projection regressions.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, Event, EventKind, EventPayload, EventSequence, EventTimestamp, Events};
use seismograph::recorder::thread::{ThreadId, ThreadLog};

fn event(kind: EventKind, id: u64, thread: u64, actor: u64) -> Event {
    Event {
        thread_id: ThreadId::new(thread),
        sequence: EventSequence::new(1),
        timestamp: EventTimestamp::from_ticks(1),
        kind,
        payload: EventPayload::Allocation(Allocation {
            allocation_id: AllocationId::new(id),
            event_thread_id: EventThreadId::new(actor),
            heap_id: HeapId::new(19),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(0x1234),
            size: 17,
            alignment: 8,
        }),
        call_stack: vec![Address::new(0x5678)],
    }
}

#[test]
fn caller_projection_matches_remote_frees_and_maps_allocator_actor_names() {
    let runtime = Events {
        total_events: 3,
        lost_events: 2,
        events: vec![
            event(EventKind::Allocation, 1, 7, 70),
            event(EventKind::Deallocation, 1, 9, 90),
            event(EventKind::Deallocation, 2, 9, 90),
        ],
        threads: vec![
            ThreadLog {
                thread_id: ThreadId::new(7),
                name: "owner".into(),
                total_events: 1,
                lost_events: 0,
            },
            ThreadLog {
                thread_id: ThreadId::new(9),
                name: "remote".into(),
                total_events: 2,
                lost_events: 2,
            },
        ],
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_eq!((projected.total_events, projected.lost_events, projected.events.len()), (3, 2, 3));
    assert_eq!(projected.events[1].thread_log_id, 7);
    assert_eq!(
        projected.events[2].thread_log_id, 9,
        "a cross-interval orphan free keeps the actual recorder log"
    );
    assert_eq!(projected.threads[0].allocated_histogram[5], 1);
    assert_eq!(projected.threads[0].live_histogram.iter().sum::<u64>(), 0);
    assert_eq!(
        projected
            .thread_names
            .iter()
            .map(|name| (name.thread_id, name.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(70, "owner"), (90, "remote")]
    );
}

#[test]
fn repeated_address_keys_pair_each_lifetime_with_its_own_owner() {
    let runtime = Events {
        events: vec![
            event(EventKind::Allocation, 0x1234, 7, 0),
            event(EventKind::Deallocation, 0x1234, 9, 0),
            event(EventKind::Allocation, 0x1234, 9, 0),
            event(EventKind::Deallocation, 0x1234, 7, 0),
            event(EventKind::Allocation, 0x1234, 7, 0),
        ],
        threads: vec![
            ThreadLog {
                thread_id: ThreadId::new(7),
                name: "first".into(),
                total_events: 3,
                lost_events: 0,
            },
            ThreadLog {
                thread_id: ThreadId::new(9),
                name: "second".into(),
                total_events: 2,
                lost_events: 0,
            },
        ],
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_eq!(projected.events[0].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[2].allocation_id, projected.events[3].allocation_id);
    assert_ne!(projected.events[0].allocation_id, projected.events[2].allocation_id);
    assert_ne!(projected.events[2].allocation_id, projected.events[4].allocation_id);
    assert_eq!(projected.events[1].thread_log_id, 7);
    assert_eq!(projected.events[3].thread_log_id, 9);
    assert_eq!(projected.events[1].event_thread_id, 9);
    assert_eq!(projected.threads[0].allocated_histogram[5], 2);
    assert_eq!(projected.threads[0].live_histogram[5], 1);
    assert_eq!(projected.threads[1].live_histogram[5], 0);
    assert_eq!(
        projected
            .thread_names
            .iter()
            .map(|thread| (thread.thread_id, thread.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(7, "first"), (9, "second")]
    );
}

#[test]
fn orphan_frees_do_not_match_later_allocations_or_other_addresses() {
    let mut other_address = event(EventKind::Deallocation, 0x1234, 9, 0);
    if let EventPayload::Allocation(allocation) = &mut other_address.payload {
        allocation.address = Address::new(0x9999);
    }
    let runtime = Events {
        events: vec![
            event(EventKind::Deallocation, 0x1234, 9, 0),
            event(EventKind::Allocation, 0x1234, 7, 0),
            other_address,
            event(EventKind::Deallocation, 0x1234, 9, 0),
            event(EventKind::Deallocation, 0x1234, 9, 0),
        ],
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_ne!(projected.events[0].allocation_id, projected.events[1].allocation_id);
    assert_ne!(projected.events[2].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[3].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[3].thread_log_id, 7);
    assert_ne!(projected.events[4].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[4].thread_log_id, 9);
}

#[test]
fn missing_free_before_reuse_does_not_merge_retained_allocation_records() {
    let runtime = Events {
        events: vec![
            event(EventKind::Allocation, 0x1234, 7, 0),
            event(EventKind::Allocation, 0x1234, 9, 0),
            event(EventKind::Deallocation, 0x1234, 7, 0),
        ],
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_ne!(projected.events[0].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[1].allocation_id, projected.events[2].allocation_id);
    assert_eq!(projected.events[2].thread_log_id, 9);
}
