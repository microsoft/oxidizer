// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Viewer-only attribution of retained operations to observed task polls.
//!
//! Attribution uses the original, unfiltered recorder stream. Filtering then removes
//! visible operations and related history without reinterpreting missing boundaries.
//! Counts describe the actor, not allocation ownership or a task's live memory.
//!
//! Evidence uses the capture's original `Events.recording` policy, never the live
//! recorder configuration. Stop preserves that policy, so closed polls remain
//! attributable offline even after their task/runtime metadata has retired.
//! Older captures with unspecified clocks or unknown/default recording policies
//! cannot prove unsampled runtime boundaries and remain conservatively unassigned
//! or ambiguous. Source activity is optional and only bounds coherent open polls.

mod attribution;
mod stacks;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use seismograph::recorder::event::{Event, EventKind, EventPayload, Events};
use seismograph_rallocator::callers::AddressLookup;
use seismograph_runtime::snapshot::Snapshot;

use super::data::{AllocationStackFilter, ThreadOperationKind};

type TaskKey = (u64, u64);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TaskEventsSnapshot {
    pub(super) tasks: BTreeMap<TaskKey, TaskEventSummary>,
    /// Supported operation events with no bounded poll evidence.
    pub(super) unassigned_events: u64,
    /// Supported operation events inside conflicting or incomplete evidence.
    pub(super) ambiguous_events: u64,
    histories: Vec<ObjectHistory>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TaskEventSummary {
    /// Only events whose inferred actor is this task; never related history.
    pub(super) inferred_events: u64,
    pub(super) operations: Vec<TaskOperation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TaskOperation {
    pub(super) kind: ThreadOperationKind,
    pub(super) events: u64,
    pub(super) objects: Vec<TaskObject>,
    /// Chronological references to only this task's selected operation, not related history.
    occurrences: Vec<TaskOccurrence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TaskOccurrence {
    object: usize,
    event: usize,
}

impl TaskOperation {
    pub(super) fn occurrences(&self) -> impl ExactSizeIterator<Item = (&TaskObject, &TaskEvent)> {
        self.occurrences.iter().map(|occurrence| {
            let object = &self.objects[occurrence.object];
            (object, &object.history[occurrence.event])
        })
    }

    pub(super) fn occurrence(&self, index: usize) -> Option<(&TaskObject, &TaskEvent)> {
        let occurrence = self.occurrences.get(index)?;
        let object = &self.objects[occurrence.object];
        Some((object, &object.history[occurrence.event]))
    }

    fn sort_occurrences(&mut self) {
        self.occurrences.sort_unstable_by_key(|occurrence| {
            let event = &self.objects[occurrence.object].history[occurrence.event];
            (event.timestamp, event.thread_id, event.sequence)
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TaskObject {
    pub(super) object_id: u64,
    /// This task's events of the selected operation kind.
    pub(super) events: u64,
    /// Address identities are explicitly not a guarantee of one object lifetime.
    pub(super) identity_note: &'static str,
    pub(super) history: Arc<[TaskEvent]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TaskEvent {
    pub(super) thread_id: u64,
    pub(super) sequence: u64,
    pub(super) timestamp: u64,
    /// Inferred actor, which may differ from the task browsing this history.
    pub(super) task: Option<TaskKey>,
    pub(super) kind: EventKind,
    pub(super) detail: String,
    pub(super) relative_stack_known: bool,
    /// No actor is assigned when retained poll evidence conflicts or has holes.
    pub(super) ambiguous: bool,
    operation: ThreadOperationKind,
    frames: Arc<stacks::Frames>,
}

impl TaskEvent {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.frames.application,
            AllocationStackFilter::All => &self.frames.complete,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObjectHistory {
    object_id: u64,
    identity_note: &'static str,
    events: Arc<[TaskEvent]>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Family {
    Allocation,
    Arc,
    Mutex,
    RwLock,
    Barrier,
    Condvar,
    Once,
    Channel,
    // Poison events do not identify the lock family. Do not silently join a
    // mutex with a reader-writer lock that happened to reuse the same address.
    UnknownLock,
}

impl Family {
    fn of(kind: ThreadOperationKind) -> Self {
        use ThreadOperationKind as K;
        match kind {
            K::Allocation | K::Deallocation => Self::Allocation,
            K::ArcCreate | K::ArcClone | K::ArcDeref | K::ArcDrop | K::ArcRelocate => Self::Arc,
            K::MutexAccess | K::MutexContention | K::MutexRelease => Self::Mutex,
            K::RwLockReadAccess
            | K::RwLockReadContention
            | K::RwLockReadRelease
            | K::RwLockWriteAccess
            | K::RwLockWriteContention
            | K::RwLockWriteRelease => Self::RwLock,
            K::BarrierAccess | K::BarrierContention | K::BarrierRelease => Self::Barrier,
            K::CondvarAccess | K::CondvarContention | K::CondvarNotify => Self::Condvar,
            K::OnceAccess | K::OnceContention | K::OnceInitialize => Self::Once,
            K::ChannelSend
            | K::ChannelSendContention
            | K::ChannelReceive
            | K::ChannelReceiveContention
            | K::ChannelClose
            | K::ChannelHighWatermark => Self::Channel,
            K::LockPoisoned | K::LockPoisonObserved | K::LockPoisonCleared => Self::UnknownLock,
        }
    }

    const fn identity_note(self) -> &'static str {
        match self {
            Self::Allocation => "Stable allocation ID and matching heap/address/layout; freeing actor is not allocation ownership",
            Self::Arc => "Arc address observations split at retained create/drop boundaries; missing lifetimes may hide address reuse",
            Self::UnknownLock => "Lock address observations; lock family and object lifetime are not recorded",
            _ => "Same-family address observations, not a guaranteed single object lifetime; address reuse may be present",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ObjectKey {
    family: Family,
    id: u64,
    /// Stable allocation IDs are additionally checked against their payload.
    allocation: Option<(u64, u64, u64, u64)>,
}

impl TaskEventsSnapshot {
    pub(super) fn from_events(events: &Events, addresses: &[AddressLookup], source: Option<&Snapshot>) -> Self {
        let evidence = attribution::infer(events, source);
        let mut stacks = stacks::Cache::new(addresses);
        let mut operations = [None; 256];
        for kind in ThreadOperationKind::ALL {
            operations[usize::from(kind.event_kind().wire_value())] = Some(kind);
        }
        let mut objects = BTreeMap::<ObjectKey, Vec<TaskEvent>>::new();
        for (index, event) in events.events.iter().enumerate() {
            let Some(operation) = operations[usize::from(event.kind.wire_value())] else {
                continue;
            };
            let Some(id) = event.object_id() else {
                continue;
            };
            let actor = evidence[index];
            let boundary = actor.boundary.map_or(&[][..], |boundary| boundary.stack(&events.events));
            let frames = stacks.get(&event.call_stack, boundary, operation, actor.task.is_some());
            let key = ObjectKey {
                family: Family::of(operation),
                id: id.get(),
                allocation: event.allocation().map(|allocation| {
                    (
                        allocation.heap_id.get(),
                        allocation.address.get(),
                        allocation.size,
                        allocation.alignment,
                    )
                }),
            };
            objects.entry(key).or_default().push(TaskEvent {
                thread_id: event.thread_id.get(),
                sequence: event.sequence.get(),
                timestamp: event.timestamp.ticks(),
                task: actor.task,
                kind: event.kind,
                detail: detail(event),
                relative_stack_known: frames.relative_known,
                operation,
                ambiguous: actor.ambiguous,
                frames,
            });
        }
        let histories = objects
            .into_iter()
            .flat_map(|(key, mut events)| {
                // Across threads this is display order, not a happens-before relation.
                events.sort_unstable_by_key(|event| (event.timestamp, event.thread_id, event.sequence));
                split_lifetimes(key, events)
            })
            .collect();
        Self::from_histories(histories)
    }

    /// Hide related records using the same event filter as the rest of the viewer,
    /// but retain actor/stack evidence computed before any boundaries were removed.
    pub(super) fn filtered(&self, visible: &HashSet<(u64, u64)>) -> Self {
        let histories = self
            .histories
            .iter()
            .filter_map(|history| {
                let shown = |event: &&TaskEvent| visible.contains(&(event.thread_id, event.sequence));
                let count = history.events.iter().filter(shown).count();
                if count == 0 {
                    return None;
                }
                let events = if count == history.events.len() {
                    Arc::clone(&history.events)
                } else {
                    history.events.iter().filter(shown).cloned().collect::<Arc<[_]>>()
                };
                Some(ObjectHistory { events, ..history.clone() })
            })
            .collect();
        Self::from_histories(histories)
    }

    fn from_histories(histories: Vec<ObjectHistory>) -> Self {
        let mut tasks = BTreeMap::<TaskKey, BTreeMap<ThreadOperationKind, TaskOperation>>::new();
        let mut unassigned_events = 0;
        let mut ambiguous_events = 0;
        for history in &histories {
            let mut occurrences = BTreeMap::<(TaskKey, ThreadOperationKind), Vec<usize>>::new();
            for (index, event) in history.events.iter().enumerate() {
                if let Some(task) = event.task {
                    occurrences.entry((task, event.operation)).or_default().push(index);
                } else if event.ambiguous {
                    ambiguous_events += 1;
                } else {
                    unassigned_events += 1;
                }
            }
            for ((task, kind), indices) in occurrences {
                let operation = tasks.entry(task).or_default().entry(kind).or_insert_with(|| TaskOperation {
                    kind,
                    events: 0,
                    objects: Vec::new(),
                    occurrences: Vec::new(),
                });
                let count = u64::try_from(indices.len()).unwrap_or(u64::MAX);
                operation.events += count;
                let object = operation.objects.len();
                operation
                    .occurrences
                    .extend(indices.into_iter().map(|event| TaskOccurrence { object, event }));
                operation.objects.push(TaskObject {
                    object_id: history.object_id,
                    events: count,
                    identity_note: history.identity_note,
                    history: Arc::clone(&history.events),
                });
            }
        }
        Self {
            tasks: tasks
                .into_iter()
                .map(|(key, operations)| {
                    let operations = operations
                        .into_values()
                        .map(|mut operation| {
                            operation.sort_occurrences();
                            operation
                        })
                        .collect::<Vec<_>>();
                    let inferred_events = operations.iter().map(|operation| operation.events).sum();
                    (
                        key,
                        TaskEventSummary {
                            inferred_events,
                            operations,
                        },
                    )
                })
                .collect(),
            unassigned_events,
            ambiguous_events,
            histories,
        }
    }
}

fn detail(event: &Event) -> String {
    match event.payload {
        EventPayload::Allocation(allocation) => format!(
            "requested={} B align={} address=0x{:x} heap={} allocator-thread={}{}",
            allocation.size,
            allocation.alignment,
            allocation.address.get(),
            allocation.heap_id.get(),
            allocation.event_thread_id.get(),
            if allocation.freed_after_heap_release {
                " after heap release"
            } else {
                ""
            },
        ),
        EventPayload::Numeric(value) => format!("{:?}; value={}", event.kind, value.value),
        _ => format!("{:?}; address observation (lifetime not recorded)", event.kind),
    }
}

fn split_lifetimes(key: ObjectKey, events: Vec<TaskEvent>) -> Vec<ObjectHistory> {
    if !matches!(key.family, Family::Arc | Family::Allocation) {
        return vec![ObjectHistory {
            object_id: key.id,
            identity_note: key.family.identity_note(),
            events: events.into(),
        }];
    }
    let create = if key.family == Family::Arc {
        EventKind::ArcCreate
    } else {
        EventKind::Allocation
    };
    let destroy = if key.family == Family::Arc {
        EventKind::ArcDrop
    } else {
        EventKind::Deallocation
    };
    let mut histories = Vec::new();
    let mut current = Vec::new();
    let mut events = events.into_iter().peekable();
    while let Some(first) = events.next() {
        let mut tied = vec![first];
        while events.peek().is_some_and(|event| event.timestamp == tied[0].timestamp) {
            if let Some(event) = events.next() {
                tied.push(event);
            }
        }
        let unordered_boundary = tied.iter().any(|event| event.thread_id != tied[0].thread_id)
            && tied.iter().any(|event| event.kind == create || event.kind == destroy);
        for event in tied {
            if unordered_boundary || event.kind == create {
                finish_history(&mut histories, &mut current, key);
            }
            let closes = unordered_boundary || event.kind == destroy;
            current.push(event);
            if closes {
                finish_history(&mut histories, &mut current, key);
            }
        }
    }
    finish_history(&mut histories, &mut current, key);
    histories
}

fn finish_history(histories: &mut Vec<ObjectHistory>, events: &mut Vec<TaskEvent>, key: ObjectKey) {
    if !events.is_empty() {
        histories.push(ObjectHistory {
            object_id: key.id,
            identity_note: key.family.identity_note(),
            events: std::mem::take(events).into(),
        });
    }
}

#[cfg(test)]
mod tests;
