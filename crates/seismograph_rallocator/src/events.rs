// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event-only projections for the v4 source; no native allocator state is inferred.

use std::collections::{HashMap, HashSet};

use seismograph::recorder::event::{EventKind as RuntimeKind, Events};

use crate::callers::{Callers, Event, EventKind, HeapKind, ThreadLog, ThreadName};

/// Projects authoritative container events into the legacy caller-view model.
///
/// Unmatched allocation records are not proof of live memory. Counterpart records
/// may be absent across intervals, suppression, sampling or buffer overwrites.
/// Address correlation keys can repeat. View-local IDs pair only the most recent
/// preceding retained allocation with a free; they do not survive captures.
#[must_use]
pub fn callers(events: &Events) -> Callers {
    let mut pending = HashMap::new();
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
    for (index, event) in events.events.iter().enumerate() {
        let Some(allocation) = event.allocation() else { continue };
        if event.kind != RuntimeKind::Allocation && event.kind != RuntimeKind::Deallocation {
            continue;
        }
        let heap_kind = match allocation.heap_kind {
            seismograph::recorder::alloc::HeapKind::General => HeapKind::General,
            seismograph::recorder::alloc::HeapKind::Bump => HeapKind::Bump,
            seismograph::recorder::alloc::HeapKind::Thread => HeapKind::Thread,
            _ => continue,
        };
        let key = (allocation.allocation_id, allocation.address);
        let identity = (index as u64 + 1, event.thread_id.get());
        let ((allocation_id, thread_log_id), allocation_recorded) = if event.kind == RuntimeKind::Allocation {
            pending.insert(key, identity);
            (identity, true)
        } else if let Some(identity) = pending.remove(&key) {
            deallocated.insert(identity.0);
            (identity, true)
        } else {
            (identity, false)
        };
        let projected = Event {
            allocation_recorded,
            thread_log_id,
            event_thread_id: if allocation.event_thread_id.get() == 0 {
                event.thread_id.get()
            } else {
                allocation.event_thread_id.get()
            },
            sequence: event.sequence.get(),
            allocation_id,
            kind: if event.kind == RuntimeKind::Allocation {
                EventKind::Allocated
            } else {
                EventKind::Deallocated
            },
            heap_id: allocation.heap_id.get(),
            heap_kind,
            freed_after_heap_release: allocation.freed_after_heap_release,
            address: allocation.address.get(),
            size: allocation.size,
            align: allocation.alignment,
            call_stack: event.call_stack.iter().map(|address| address.get()).collect(),
        };
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
        result.events.push(projected);
    }
    for event in &result.events {
        if event.kind == EventKind::Allocated
            && !deallocated.contains(&event.allocation_id)
            && let Some(index) = log_indexes.get(&event.thread_log_id)
        {
            let bucket = (u64::BITS - event.size.leading_zeros()) as usize;
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
