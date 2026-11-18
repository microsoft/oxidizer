// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authoritative container allocation projection regressions.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind, Reallocation};
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

fn ordered(mut events: Vec<Event>) -> Vec<Event> {
    for (index, event) in events.iter_mut().enumerate() {
        event.timestamp = EventTimestamp::from_ticks(index as u64);
        event.sequence = EventSequence::new(index as u64 + 1);
    }
    events
}

fn resize(old: u64, new: u64, old_size: u64, new_size: u64, thread: u64) -> Event {
    Event {
        kind: EventKind::Reallocation,
        payload: EventPayload::Reallocation(Reallocation {
            allocation_id: AllocationId::new(old),
            heap_id: HeapId::new(19),
            heap_kind: HeapKind::General,
            old_address: Address::new(old),
            new_address: Address::new(new),
            old_size,
            new_size,
            alignment: 8,
        }),
        ..event(EventKind::Allocation, old, thread, 0)
    }
}

#[test]
fn moving_resize_preserves_its_origin_without_consuming_a_reused_old_address() {
    let old = 0x1234;
    let new = 0x2000;
    let mut moved_free = event(EventKind::Deallocation, new, 9, 0);
    if let EventPayload::Allocation(allocation) = &mut moved_free.payload {
        allocation.address = Address::new(new);
        allocation.size = 4096;
    }
    let input = Events {
        events: ordered(vec![
            event(EventKind::Allocation, old, 7, 0),
            resize(old, new, 17, 4096, 9),
            event(EventKind::Allocation, old, 7, 0),
            moved_free,
            event(EventKind::Deallocation, old, 7, 0),
        ]),
        ..Events::default()
    };

    let projected = seismograph_rallocator::events::callers(&input);
    let events = &projected.events;

    assert_eq!(
        (
            events[0].allocation_id,
            events[1].allocation_id,
            events[3].allocation_id,
            events[1].thread_log_id,
            events[1].event_thread_id,
            events[0].resized_size,
        ),
        (
            events[0].allocation_id,
            events[0].allocation_id,
            events[0].allocation_id,
            7,
            9,
            Some(4096),
        )
    );
    assert_eq!(events[2].allocation_id, events[4].allocation_id);
    assert_ne!(events[0].allocation_id, events[2].allocation_id);
    assert_eq!(events[1].kind, seismograph_rallocator::callers::EventKind::Reallocated);
    assert_eq!(
        events[1].reallocation,
        Some(seismograph_rallocator::callers::Resize {
            old_address: old,
            old_size: 17,
        })
    );
}

#[test]
fn in_place_resizes_update_retained_size_without_inventing_allocation_counts() {
    let input = Events {
        events: ordered(vec![
            event(EventKind::Allocation, 0x1234, 7, 0),
            resize(0x1234, 0x1234, 17, 128, 9),
            resize(0x1234, 0x1234, 128, 64, 9),
        ]),
        threads: vec![ThreadLog {
            thread_id: ThreadId::new(7),
            name: "origin".into(),
            total_events: 1,
            lost_events: 0,
        }],
        ..Events::default()
    };

    let projected = seismograph_rallocator::events::callers(&input);

    assert_eq!(
        (
            projected.threads[0].allocated_histogram.iter().sum::<u64>(),
            projected.threads[0].live_histogram[7],
            projected.events[0].resized_size,
        ),
        (1, 1, Some(64))
    );
    assert!(
        projected
            .events
            .iter()
            .all(|event| event.allocation_id == projected.events[0].allocation_id)
    );
}

#[test]
fn orphan_resize_keeps_an_unknown_origin_when_its_new_storage_is_freed() {
    let mut free = event(EventKind::Deallocation, 0x2000, 9, 0);
    if let EventPayload::Allocation(allocation) = &mut free.payload {
        allocation.address = Address::new(0x2000);
        allocation.size = 4096;
    }
    let input = Events {
        events: ordered(vec![resize(0x1234, 0x2000, 17, 4096, 7), free]),
        ..Events::default()
    };

    let projected = seismograph_rallocator::events::callers(&input);

    assert_eq!(projected.events[0].allocation_id, projected.events[1].allocation_id);
    assert!(projected.events.iter().all(|event| !event.allocation_recorded));
}
#[test]
fn stable_ids_follow_chronological_moves_while_preserving_input_and_recorder_identity() {
    let mut first_resize = resize(0x1234, 0x2000, 17, 32, 9);
    let mut second_resize = resize(0x2000, 0x3000, 32, 80, 9);
    for event in [&mut first_resize, &mut second_resize] {
        let EventPayload::Reallocation(resize) = &mut event.payload else {
            unreachable!();
        };
        resize.allocation_id = AllocationId::new(55);
    }
    let mut free = event(EventKind::Deallocation, 55, 9, 90);
    if let EventPayload::Allocation(allocation) = &mut free.payload {
        allocation.address = Address::new(0x3000);
        allocation.size = 80;
    }
    let mut stale_free = event(EventKind::Deallocation, 55, 9, 90);
    if let EventPayload::Allocation(allocation) = &mut stale_free.payload {
        allocation.address = Address::new(0x2000);
        allocation.size = 32;
    }
    let chronological = ordered(vec![
        event(EventKind::Allocation, 55, 7, 70),
        first_resize,
        second_resize,
        free,
        stale_free,
    ]);
    let input = Events {
        events: [3, 2, 0, 1, 4].map(|index| chronological[index].clone()).into(),
        ..Events::default()
    };

    let projected = seismograph_rallocator::events::callers(&input);
    let events = &projected.events;

    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            seismograph_rallocator::callers::EventKind::Deallocated,
            seismograph_rallocator::callers::EventKind::Reallocated,
            seismograph_rallocator::callers::EventKind::Allocated,
            seismograph_rallocator::callers::EventKind::Reallocated,
            seismograph_rallocator::callers::EventKind::Deallocated,
        ]
    );
    assert!(events[..4].iter().all(|event| {
        event.allocation_id == events[2].allocation_id
            && event.thread_log_id == 7
            && event.allocation_recorded
            && event.resized_size == Some(80)
    }));
    assert_eq!((events[0].recorder_thread_id, events[0].event_thread_id), (9, 90));
    assert_ne!(events[0].allocation_id, events[4].allocation_id);
    assert!(!events[4].allocation_recorded);
}

#[test]
fn gamma_survivor_projection_distinguishes_orphans_pairing_and_input_identity() {
    let mut ignored = event(EventKind::TaskSpawned, 1, 7, 0);
    ignored.payload = EventPayload::Object(seismograph::recorder::event::ObjectId::new(1));
    let runtime = Events {
        total_events: 11,
        lost_events: 3,
        events: ordered(vec![
            ignored,
            event(EventKind::TaskSpawned, 1, 7, 0),
            event(EventKind::Deallocation, 1, 9, 90),
            event(EventKind::Allocation, 1, 7, 70),
            event(EventKind::Deallocation, 1, 9, 90),
            event(EventKind::Allocation, 2, 7, 70),
        ]),
        threads: vec![
            ThreadLog {
                thread_id: ThreadId::new(7),
                name: "owner".into(),
                total_events: 2,
                lost_events: 1,
            },
            ThreadLog {
                thread_id: ThreadId::new(9),
                name: "remote".into(),
                total_events: 2,
                lost_events: 0,
            },
        ],
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_eq!((projected.total_events, projected.lost_events, projected.events.len()), (11, 3, 4));
    assert_eq!(
        projected
            .events
            .iter()
            .map(|event| (event.allocation_id, event.thread_log_id, event.allocation_recorded))
            .collect::<Vec<_>>(),
        [(3, 9, false), (4, 7, true), (4, 7, true), (6, 7, true)]
    );
    assert_eq!(projected.threads[0].allocated_histogram[5], 2);
    assert_eq!(projected.threads[0].live_histogram[5], 1);
    assert_eq!(projected.threads[1].live_histogram[5], 0);
    assert_eq!(projected.events[1].call_stack, [0x5678]);
    assert_eq!(projected.events[2].event_thread_id, 90);
}

#[test]
fn gamma_survivor_field_adapters_preserve_nondefault_values() {
    use seismograph_rallocator::callers::{AddressLookup, AddressLookupFields, Callers, CallersFields, ThreadName, ThreadNameFields};
    let callers = Callers::from_fields(CallersFields {
        session_id: 13,
        total_events: 17,
        lost_events: 19,
        threads: Vec::new(),
        events: Vec::new(),
        thread_names: vec![ThreadName::from_fields(ThreadNameFields {
            thread_id: 23,
            name: "worker".into(),
        })],
    });
    assert_eq!((callers.session_id, callers.total_events, callers.lost_events), (13, 17, 19));
    assert_eq!(
        (callers.thread_names[0].thread_id, callers.thread_names[0].name.as_str()),
        (23, "worker")
    );
    let address = AddressLookup::from_fields(AddressLookupFields {
        address: 29,
        symbol: Some("symbol".into()),
        filename: Some("source.rs".into()),
        line: Some(31),
        column: Some(37),
    });
    assert_eq!((address.address, address.line, address.column), (29, Some(31), Some(37)));
    assert_eq!(
        (address.symbol.as_deref(), address.filename.as_deref()),
        (Some("symbol"), Some("source.rs"))
    );
}
#[test]
fn caller_projection_preserves_all_known_heap_kinds_and_ignores_nonallocation_kinds() {
    let mut events = Vec::new();
    for (id, kind) in [(1, HeapKind::General), (2, HeapKind::Bump), (3, HeapKind::Thread)] {
        let mut event = event(EventKind::Allocation, id, 7, 0);
        let EventPayload::Allocation(allocation) = &mut event.payload else {
            unreachable!();
        };
        allocation.heap_kind = kind;
        events.push(event);
    }
    let mut nonallocation = events[0].clone();
    nonallocation.kind = EventKind::TaskSpawned;
    events.push(nonallocation);
    let projected = seismograph_rallocator::events::callers(&Events {
        events,
        ..Default::default()
    });
    assert_eq!(projected.events.len(), 3);
    assert_eq!(projected.events[0].heap_kind, seismograph_rallocator::callers::HeapKind::General);
    assert_eq!(projected.events[1].heap_kind, seismograph_rallocator::callers::HeapKind::Bump);
    assert_eq!(projected.events[2].heap_kind, seismograph_rallocator::callers::HeapKind::Thread);
}

#[test]
fn caller_field_constructors_preserve_log_and_actor_metadata() {
    use seismograph_rallocator::callers::{ThreadLog as CallerLog, ThreadLogFields, ThreadName, ThreadNameFields};

    let log = CallerLog::from_fields(ThreadLogFields {
        thread_log_id: 7,
        total_events: 9,
        lost_events: 2,
        allocated_histogram: vec![3, 4],
        live_histogram: vec![1, 2],
    });
    assert_eq!((log.thread_log_id, log.total_events, log.lost_events), (7, 9, 2));
    assert_eq!(log.allocated_histogram, [3, 4]);
    assert_eq!(log.live_histogram, [1, 2]);
    let name = ThreadName::from_fields(ThreadNameFields {
        thread_id: 70,
        name: "remote actor".into(),
    });
    assert_eq!((name.thread_id, name.name.as_str()), (70, "remote actor"));
}

#[test]
fn caller_projection_matches_remote_frees_and_maps_allocator_actor_names() {
    let runtime = Events {
        total_events: 3,
        lost_events: 2,
        events: ordered(vec![
            event(EventKind::Allocation, 1, 7, 70),
            event(EventKind::Deallocation, 1, 9, 90),
            event(EventKind::Deallocation, 2, 9, 90),
        ]),
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
        events: ordered(vec![
            event(EventKind::Allocation, 0x1234, 7, 0),
            event(EventKind::Deallocation, 0x1234, 9, 0),
            event(EventKind::Allocation, 0x1234, 9, 0),
            event(EventKind::Deallocation, 0x1234, 7, 0),
            event(EventKind::Allocation, 0x1234, 7, 0),
        ]),
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
        events: ordered(vec![
            event(EventKind::Deallocation, 0x1234, 9, 0),
            event(EventKind::Allocation, 0x1234, 7, 0),
            other_address,
            event(EventKind::Deallocation, 0x1234, 9, 0),
            event(EventKind::Deallocation, 0x1234, 9, 0),
        ]),
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
        events: ordered(vec![
            event(EventKind::Allocation, 0x1234, 7, 0),
            event(EventKind::Allocation, 0x1234, 9, 0),
            event(EventKind::Deallocation, 0x1234, 7, 0),
        ]),
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_ne!(projected.events[0].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[1].allocation_id, projected.events[2].allocation_id);
    assert_eq!(projected.events[2].thread_log_id, 9);
}

#[test]
fn chronological_pairing_preserves_recorder_order_and_breaks_timestamp_ties() {
    let mut allocation = event(EventKind::Allocation, 0x1234, 7, 0);
    let mut free = event(EventKind::Deallocation, 0x1234, 9, 0);
    allocation.timestamp = EventTimestamp::from_ticks(2);
    free.timestamp = EventTimestamp::from_ticks(3);
    let mut reused = event(EventKind::Allocation, 0x1234, 9, 0);
    reused.timestamp = EventTimestamp::from_ticks(3);
    reused.sequence = EventSequence::new(2);
    let runtime = Events {
        events: vec![free, reused, allocation],
        ..Default::default()
    };
    let projected = seismograph_rallocator::events::callers(&runtime);
    assert_eq!(projected.events[0].kind, seismograph_rallocator::callers::EventKind::Deallocated);
    assert_eq!(projected.events[0].allocation_id, projected.events[2].allocation_id);
    assert_ne!(projected.events[0].allocation_id, projected.events[1].allocation_id);
    assert_eq!(projected.events[0].thread_log_id, 7);
    assert!(projected.events[0].allocation_recorded);
}
