// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event-only projections for the v4 source; no native allocator state is inferred.

use std::collections::{HashMap, HashSet};

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId};
use seismograph::recorder::event::{Address, Event as RuntimeEvent, EventKind as RuntimeKind, Events};

use crate::callers::{Callers, Event, EventKind, HeapKind, Resize, ThreadLog, ThreadName};

const HEAP_KINDS: [(seismograph::recorder::alloc::HeapKind, HeapKind); 3] = [
    (seismograph::recorder::alloc::HeapKind::General, HeapKind::General),
    (seismograph::recorder::alloc::HeapKind::Bump, HeapKind::Bump),
    (seismograph::recorder::alloc::HeapKind::Thread, HeapKind::Thread),
];

type CorrelationKey = (AllocationId, Address);
type ProjectedInput<'a> = (usize, &'a RuntimeEvent, Allocation, HeapKind, Option<Resize>);

fn chronological_allocations(events: &Events) -> Vec<ProjectedInput<'_>> {
    let mut allocations = events
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            if !matches!(
                event.kind,
                RuntimeKind::Allocation | RuntimeKind::Deallocation | RuntimeKind::Reallocation
            ) {
                return None;
            }
            let (allocation, resize) = if let Some(resize) = event.reallocation() {
                (
                    Allocation {
                        allocation_id: resize.allocation_id,
                        event_thread_id: EventThreadId::new(0),
                        heap_id: resize.heap_id,
                        heap_kind: resize.heap_kind,
                        freed_after_heap_release: false,
                        address: resize.new_address,
                        size: resize.new_size,
                        alignment: resize.alignment,
                    },
                    Some(Resize {
                        old_address: resize.old_address.get(),
                        old_size: resize.old_size,
                    }),
                )
            } else {
                (event.allocation()?, None)
            };
            HEAP_KINDS.iter().find_map(|(runtime, projected)| {
                (*runtime == allocation.heap_kind).then_some((index, event, allocation, *projected, resize))
            })
        })
        .collect::<Vec<_>>();
    allocations.sort_by_key(|(_, event, _, _, _)| (event.timestamp.ticks(), event.thread_id.get(), event.sequence.get()));
    allocations
}

struct Lifetime {
    thread_log_id: u64,
    allocation_recorded: bool,
    keys: [CorrelationKey; 2],
    resized_size: Option<u64>,
}

impl Lifetime {
    fn project(&self, identity: u64, event: &RuntimeEvent, allocation: Allocation, heap_kind: HeapKind, resize: Option<Resize>) -> Event {
        let kind = match event.kind {
            RuntimeKind::Allocation => EventKind::Allocated,
            RuntimeKind::Reallocation => EventKind::Reallocated,
            _ => EventKind::Deallocated,
        };
        Event {
            allocation_recorded: self.allocation_recorded,
            thread_log_id: self.thread_log_id,
            recorder_thread_id: event.thread_id.get(),
            event_thread_id: if allocation.event_thread_id.get() == 0 {
                event.thread_id.get()
            } else {
                allocation.event_thread_id.get()
            },
            sequence: event.sequence.get(),
            allocation_id: identity,
            kind,
            heap_id: allocation.heap_id.get(),
            heap_kind,
            freed_after_heap_release: allocation.freed_after_heap_release,
            address: allocation.address.get(),
            size: allocation.size,
            align: allocation.alignment,
            reallocation: resize,
            resized_size: None,
            call_stack: event.call_stack.iter().map(|address| address.get()).collect(),
        }
    }
}

fn remove_keys(pending: &mut HashMap<CorrelationKey, u64>, lifetime: &Lifetime, identity: u64) {
    for key in lifetime.keys {
        if pending.get(&key) == Some(&identity) {
            pending.remove(&key);
        }
    }
}

/// Projects authoritative container events into the caller-view model.
///
/// Unmatched allocation records are not proof of live memory. Counterpart records
/// may be absent across intervals, suppression, sampling or buffer overwrites.
/// Address correlation keys can repeat. View-local IDs pair only the most recent
/// preceding retained allocation with a free. Resizes preserve that identity
/// and replace its storage keys. These identities do not survive captures.
/// Pairing uses timestamp, recorder thread, then sequence order. Exact ties retain
/// input order, and the projected events retain their original recorder order.
#[must_use]
pub fn callers(events: &Events) -> Callers {
    let mut pending = HashMap::new();
    let mut lifetimes = HashMap::<u64, Lifetime>::new();
    let mut deallocated = HashSet::new();
    let mut result = Callers {
        total_events: events.total_events,
        lost_events: events.lost_events,
        ..Default::default()
    };
    let mut log_indexes = HashMap::new();
    let recorder_names = events
        .threads
        .iter()
        .map(|thread| (thread.thread_id.get(), thread.name.as_str()))
        .collect::<HashMap<_, _>>();
    let mut actor_names = HashMap::new();
    for thread in &events.threads {
        let log = ThreadLog {
            thread_log_id: thread.thread_id.get(),
            total_events: thread.total_events,
            lost_events: thread.lost_events,
            allocated_histogram: vec![0; 65],
            live_histogram: vec![0; 65],
        };
        log_indexes.insert(log.thread_log_id, result.threads.len());
        result.threads.push(log);
    }
    let allocations = chronological_allocations(events);
    let mut projected_events = Vec::with_capacity(allocations.len());
    for (index, event, allocation, heap_kind, resize) in allocations {
        let address = resize.map_or(allocation.address, |resize| Address::new(resize.old_address));
        let old_key = (allocation.allocation_id, address);
        let allocation_id = if event.kind == RuntimeKind::Allocation {
            index as u64 + 1
        } else {
            pending.get(&old_key).copied().unwrap_or(index as u64 + 1)
        };
        let lifetime = lifetimes.entry(allocation_id).or_insert_with(|| Lifetime {
            thread_log_id: event.thread_id.get(),
            allocation_recorded: event.kind == RuntimeKind::Allocation,
            keys: [old_key; 2],
            resized_size: None,
        });
        remove_keys(&mut pending, lifetime, allocation_id);
        if event.kind == RuntimeKind::Deallocation {
            deallocated.insert(allocation_id);
        } else if resize.is_some() {
            lifetime.keys = [
                (allocation.allocation_id, allocation.address),
                (AllocationId::new(allocation.address.get()), allocation.address),
            ];
            lifetime.resized_size = Some(allocation.size);
            for key in lifetime.keys {
                pending.insert(key, allocation_id);
            }
        } else {
            pending.insert(old_key, allocation_id);
        }
        let projected = lifetime.project(allocation_id, event, allocation, heap_kind, resize);
        if let Some(name) = recorder_names.get(&event.thread_id.get()) {
            actor_names.insert(projected.event_thread_id, *name);
        }
        if projected.kind == EventKind::Allocated
            && let Some(index) = log_indexes.get(&projected.thread_log_id)
        {
            let log = &mut result.threads[*index];
            let bucket = (u64::BITS - allocation.size.leading_zeros()) as usize;
            log.allocated_histogram[bucket] += 1;
        }
        projected_events.push((index, projected));
    }
    projected_events.sort_unstable_by_key(|(index, _)| *index);
    result.events = projected_events.into_iter().map(|(_, event)| event).collect();
    for event in &mut result.events {
        event.resized_size = lifetimes.get(&event.allocation_id).and_then(|lifetime| lifetime.resized_size);
        if event.kind == EventKind::Allocated
            && !deallocated.contains(&event.allocation_id)
            && let Some(index) = log_indexes.get(&event.thread_log_id)
        {
            let size = event.resized_size.unwrap_or(event.size);
            let bucket = (u64::BITS - size.leading_zeros()) as usize;
            result.threads[*index].live_histogram[bucket] += 1;
        }
    }
    result.thread_names = actor_names
        .into_iter()
        .map(|(thread_id, name)| ThreadName {
            thread_id,
            name: name.to_owned(),
        })
        .collect();
    result.thread_names.sort_unstable_by_key(|thread| thread.thread_id);
    result
}
