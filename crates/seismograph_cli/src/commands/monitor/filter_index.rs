// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use seismograph::recorder::event::{Address, Event, EventKind, EventPayload, EventSequence, EventTimestamp};
use seismograph::recorder::thread::ThreadId;
use seismograph::snapshot::DecodedSnapshot;
use seismograph_rallocator::callers::{AddressLookup, Event as AllocationEvent, EventKind as AllocationEventKind};
use seismograph_rallocator::snapshot::Snapshot as AllocatorSource;
use seismograph_runtime::snapshot::Snapshot as RuntimeSource;

use super::data::{AllocationSnapshot, CapturedSnapshot, MemorySnapshot, RuntimeSnapshot, runtime_task_id};
use super::filter::{FilterSpec, Match, RuntimeStackMode, StackProvenance};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FilterCounts {
    pub(super) total: u64,
    pub(super) shown: u64,
    pub(super) unknown: u64,
}

impl FilterCounts {
    fn observe(&mut self, filter: &FilterSpec, classification: Match) -> bool {
        self.total += 1;
        self.unknown += u64::from(classification == Match::Unknown);
        let shown = filter.accepts(classification);
        self.shown += u64::from(shown);
        shown
    }

    fn unfiltered(total: usize) -> Self {
        let total = u64::try_from(total).unwrap_or(u64::MAX);
        Self {
            total,
            shown: total,
            unknown: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FilterSummary {
    pub(super) active: bool,
    pub(super) events: FilterCounts,
    pub(super) allocations: FilterCounts,
    pub(super) tasks: FilterCounts,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct StackId(usize);

struct Stack {
    addresses: Arc<[u64]>,
    provenance: StackProvenance,
}

#[derive(Clone, Copy)]
struct IndexedEvent {
    thread_id: ThreadId,
    sequence: EventSequence,
    timestamp: EventTimestamp,
    kind: EventKind,
    payload: EventPayload,
    stack: StackId,
}

impl IndexedEvent {
    fn task_key(self) -> Option<(u64, u64)> {
        let EventPayload::Runtime(runtime) = self.payload else {
            return None;
        };
        runtime_task_id(self.kind, runtime.subject_id, runtime.related_id).map(|task| (runtime.runtime_id.get(), task))
    }

    fn restore(self, stacks: &[Stack]) -> Event {
        Event {
            thread_id: self.thread_id,
            sequence: self.sequence,
            timestamp: self.timestamp,
            kind: self.kind,
            payload: self.payload,
            call_stack: stacks[self.stack.0].addresses.iter().copied().map(Address::new).collect(),
        }
    }
}

struct IndexedAllocation {
    event: AllocationEvent,
    stack: StackId,
}

/// Immutable replay input: stack allocations and provenance are shared by identical stacks.
pub(super) struct FilterIndex {
    header: DecodedSnapshot,
    events: Vec<IndexedEvent>,
    stacks: Vec<Stack>,
    addresses: Vec<AddressLookup>,
    allocator: Option<AllocatorSource>,
    allocations: Vec<IndexedAllocation>,
    retained_allocation_events: u64,
    deallocated: HashSet<(u64, u64)>,
    runtime: Option<RuntimeSource>,
    task_stacks: HashMap<(u64, u64), StackId>,
    source_task_stacks: HashMap<(u64, u64), StackId>,
    io_stacks: HashMap<u64, StackId>,
}

impl FilterIndex {
    pub(super) fn new(
        mut decoded: DecodedSnapshot,
        mut allocator: Option<AllocatorSource>,
        mut runtime: Option<RuntimeSource>,
        addresses: Vec<AddressLookup>,
        deallocated: HashSet<(u64, u64)>,
    ) -> Self {
        let lookups = addresses
            .iter()
            .map(|lookup| (lookup.address, lookup.symbol.as_deref()))
            .collect::<HashMap<_, _>>();
        let mut stacks = Vec::new();
        let mut interned = HashMap::<Arc<[u64]>, StackId>::new();
        let mut intern = |addresses: Vec<u64>| {
            if let Some(id) = interned.get(addresses.as_slice()) {
                return *id;
            }

            let id = StackId(stacks.len());
            let provenance = StackProvenance::from_symbols(addresses.iter().map(|address| lookups.get(address).copied().flatten()));
            let addresses: Arc<[u64]> = addresses.into();
            interned.insert(Arc::clone(&addresses), id);
            stacks.push(Stack { addresses, provenance });
            id
        };
        // Stack zero represents missing provenance for partial lifecycle records.
        intern(Vec::new());
        let mut task_stacks = HashMap::new();
        let mut source_task_stacks = HashMap::new();
        if let Some(source) = &mut runtime {
            for runtime in &mut source.runtimes {
                for task in &mut runtime.tasks {
                    let stack = intern(std::mem::take(&mut task.spawn_backtrace).into_iter().map(Address::get).collect());
                    task_stacks.insert((runtime.id.get(), task.id.get()), stack);
                    source_task_stacks.insert((runtime.id.get(), task.id.get()), stack);
                }
                for worker in &runtime.workers {
                    if let Some(task) = worker.current_task {
                        task_stacks.entry((runtime.id.get(), task.get())).or_default();
                    }
                }
            }
        }
        let mut io_frames = HashMap::<u64, Vec<u64>>::new();
        let events = std::mem::take(&mut decoded.events.events)
            .into_iter()
            .map(|event| {
                let addresses = event.call_stack.into_iter().map(Address::get).collect::<Vec<_>>();
                if let EventPayload::Io(io) = event.payload {
                    io_frames.entry(io.operation_id.get()).or_default().extend(&addresses);
                }
                let stack = intern(addresses);
                let indexed = IndexedEvent {
                    thread_id: event.thread_id,
                    sequence: event.sequence,
                    timestamp: event.timestamp,
                    kind: event.kind,
                    payload: event.payload,
                    stack,
                };
                if let Some(key) = indexed.task_key() {
                    let spawn = task_stacks.entry(key).or_default();
                    if event.kind == EventKind::TaskSpawned && *spawn == StackId::default() {
                        *spawn = stack;
                    }
                }
                indexed
            })
            .collect();
        let io_stacks = io_frames
            .into_iter()
            .map(|(operation, mut addresses)| {
                addresses.sort_unstable();
                addresses.dedup();
                (operation, intern(addresses))
            })
            .collect();
        let mut retained_allocation_events = 0;
        let allocations = allocator
            .as_mut()
            .and_then(|source| source.callers.as_mut())
            .map_or_else(Vec::new, |callers| {
                retained_allocation_events = u64::try_from(callers.events.len()).unwrap_or(u64::MAX);
                std::mem::take(&mut callers.events)
                    .into_iter()
                    .filter(|event| event.kind == AllocationEventKind::Allocated)
                    .map(|mut event| {
                        let stack = intern(std::mem::take(&mut event.call_stack));
                        IndexedAllocation { event, stack }
                    })
                    .collect()
            });
        Self {
            header: decoded,
            events,
            stacks,
            addresses,
            allocator,
            allocations,
            retained_allocation_events,
            deallocated,
            runtime,
            task_stacks,
            source_task_stacks,
            io_stacks,
        }
    }

    pub(super) fn unfiltered_summary(&self) -> FilterSummary {
        FilterSummary {
            active: false,
            events: FilterCounts::unfiltered(self.events.len()),
            allocations: FilterCounts::unfiltered(self.allocations.len()),
            tasks: FilterCounts::unfiltered(self.task_stacks.len()),
        }
    }

    pub(super) fn render(self: &Arc<Self>, filter: &FilterSpec) -> Box<CapturedSnapshot> {
        let matches = self
            .stacks
            .iter()
            .map(|stack| filter.classify(&stack.provenance))
            .collect::<Vec<_>>();
        let mut summary = FilterSummary {
            active: filter.is_active(),
            ..FilterSummary::default()
        };
        let mut visible_tasks = self
            .task_stacks
            .iter()
            .filter_map(|(key, stack)| summary.tasks.observe(filter, matches[stack.0]).then_some(*key))
            .collect::<HashSet<_>>();
        let mut decoded = DecodedSnapshot {
            capture_duration_nanos: self.header.capture_duration_nanos,
            events: self.header.events.clone(),
            sources: Vec::new(),
        };
        decoded.events.events = self
            .events
            .iter()
            .filter_map(|event| {
                let stack = self.event_provenance(*event, filter.runtime_stack);
                if !summary.events.observe(filter, matches[stack.0]) {
                    return None;
                }
                if let Some(task) = event.task_key() {
                    visible_tasks.insert(task);
                }
                Some(event.restore(&self.stacks))
            })
            .collect();
        summary.tasks.shown = u64::try_from(visible_tasks.len()).unwrap_or(u64::MAX);
        if filter.is_active() {
            let visible_threads = decoded.events.events.iter().map(|event| event.thread_id).collect::<HashSet<_>>();
            decoded.events.threads.retain(|thread| visible_threads.contains(&thread.thread_id));
        }
        let source = self.runtime_source(filter, &visible_tasks, &decoded.events.events);
        let runtime = RuntimeSnapshot::from_events_with_progress(&decoded, &self.addresses, source.as_ref(), &mut |_| {});
        super::snapshot::release_stacks(&mut decoded.events.events, |event| drop(std::mem::take(&mut event.call_stack)));
        drop(decoded);
        drop(source);

        let mut allocation_events = self
            .allocations
            .iter()
            .filter_map(|allocation| {
                if !summary.allocations.observe(filter, matches[allocation.stack.0]) {
                    return None;
                }
                let mut event = allocation.event.clone();
                event.call_stack = self.stacks[allocation.stack.0].addresses.to_vec();
                Some(event)
            })
            .collect::<Vec<_>>();
        let memory = self
            .allocator
            .as_ref()
            .map(|source| MemorySnapshot::from_snapshot_with_events(source, &self.deallocated, &allocation_events));
        let allocations = self.allocator.as_ref().map(|source| {
            let mut snapshot = AllocationSnapshot::from_snapshot_with_events(source, &self.deallocated, &allocation_events);
            if !filter.is_active() {
                snapshot.retained_events = self.retained_allocation_events;
            }
            snapshot
        });
        super::snapshot::release_stacks(&mut allocation_events, |event| drop(std::mem::take(&mut event.call_stack)));
        Box::new(CapturedSnapshot {
            memory,
            allocations,
            heap_error: self
                .allocator
                .is_none()
                .then(|| format!("heap data unavailable: {}", super::Error::MissingMemorySource)),
            primitives: runtime.primitives,
            runtime: runtime.runtime,
            io: runtime.io,
            cache: runtime.cache,
            threads: runtime.threads,
            captured_at: None,
            captured_instant: None,
            filter_index: Some(Arc::clone(self)),
            filter_summary: summary,
        })
    }

    fn event_provenance(&self, event: IndexedEvent, runtime_stack: RuntimeStackMode) -> StackId {
        if runtime_stack == RuntimeStackMode::Spawn
            && let Some(task) = event.task_key()
        {
            return self.task_stacks.get(&task).copied().unwrap_or_default();
        }
        if let EventPayload::Io(io) = event.payload {
            // Match all captured operation frames, then keep or remove the pair.
            return self.io_stacks.get(&io.operation_id.get()).copied().unwrap_or_default();
        }
        event.stack
    }

    fn runtime_source(&self, filter: &FilterSpec, visible_tasks: &HashSet<(u64, u64)>, events: &[Event]) -> Option<RuntimeSource> {
        let mut source = self.runtime.clone()?;
        let mut visible_workers = events
            .iter()
            .filter_map(|event| {
                let runtime = event.runtime()?;
                Some((runtime.runtime_id.get(), runtime.worker_id?.get()))
            })
            .collect::<HashSet<_>>();
        for runtime in &mut source.runtimes {
            if filter.is_active() {
                runtime
                    .tasks
                    .retain(|task| visible_tasks.contains(&(runtime.id.get(), task.id.get())));
                for task in &runtime.tasks {
                    if let Some(worker) = task.last_worker_id {
                        visible_workers.insert((runtime.id.get(), worker.get()));
                    }
                }
                for worker in &mut runtime.workers {
                    worker.current_task = worker
                        .current_task
                        .filter(|task| visible_tasks.contains(&(runtime.id.get(), task.get())));
                }
                runtime
                    .workers
                    .retain(|worker| worker.current_task.is_some() || visible_workers.contains(&(runtime.id.get(), worker.id.get())));
            }
            for task in &mut runtime.tasks {
                let stack = self
                    .source_task_stacks
                    .get(&(runtime.id.get(), task.id.get()))
                    .copied()
                    .unwrap_or_default();
                task.spawn_backtrace = self.stacks[stack.0].addresses.iter().copied().map(Address::new).collect();
            }
        }
        Some(source)
    }
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{Events, ObjectId};
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
    use seismograph_rallocator::callers::{AddressLookupFields, Callers};
    use seismograph_rallocator::snapshot::Version;

    use super::*;

    fn lookup(address: u64, symbol: &str) -> AddressLookup {
        AddressLookup::from_fields(AddressLookupFields {
            address,
            symbol: Some(symbol.into()),
            filename: None,
            line: None,
            column: None,
        })
    }

    fn event(sequence: u64, address: Option<u64>) -> Event {
        Event {
            thread_id: ThreadId::new(sequence),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind: EventKind::ArcClone,
            payload: EventPayload::Object(ObjectId::new(sequence)),
            call_stack: address.into_iter().map(Address::new).collect(),
        }
    }

    fn index(events: Vec<Event>, addresses: Vec<AddressLookup>) -> Arc<FilterIndex> {
        Arc::new(FilterIndex::new(
            DecodedSnapshot {
                events: Events {
                    total_events: u64::try_from(events.len()).unwrap(),
                    events,
                    ..Events::default()
                },
                ..DecodedSnapshot::default()
            },
            None,
            None,
            addresses,
            HashSet::new(),
        ))
    }

    #[test]
    fn rules_reaggregate_records_and_reset_without_recapturing() {
        let index = index(
            vec![event(1, Some(1)), event(2, Some(2)), event(3, None)],
            vec![lookup(1, "app::work"), lookup(2, "noise::work")],
        );
        let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(
            filtered.filter_summary.events,
            FilterCounts {
                total: 3,
                shown: 1,
                unknown: 1
            }
        );
        assert_eq!(filtered.primitives.groups[0].events, 1);
        assert_eq!(
            filtered.threads.threads.iter().map(|thread| thread.thread_id).collect::<Vec<_>>(),
            [1]
        );
        let with_unknown = FilterSpec::parse("crate:app", "", true, RuntimeStackMode::Event).unwrap();
        assert_eq!(index.render(&with_unknown).primitives.groups[0].events, 2);
        let reset = index.render(&FilterSpec::default());
        assert_eq!(reset.primitives.groups[0].events, 3);
        assert!(!reset.filter_summary.active);
        assert!(Arc::ptr_eq(reset.filter_index.as_ref().unwrap(), &index));
    }

    #[test]
    fn allocation_lifetimes_are_correlated_before_excluding_free_stacks() {
        let allocation = |id, size, kind, stack| {
            let mut event = AllocationEvent::default();
            event.thread_log_id = 1;
            event.allocation_id = id;
            event.size = size;
            event.align = 8;
            event.kind = kind;
            event.call_stack = vec![stack];
            event
        };
        let mut source = AllocatorSource::new(Version::new(0, 1, 0));
        source.stats.live_bytes = 777;
        let mut callers = Callers::default();
        callers.events = vec![
            allocation(1, 64, AllocationEventKind::Allocated, 1),
            allocation(1, 64, AllocationEventKind::Deallocated, 2),
            allocation(2, 128, AllocationEventKind::Allocated, 2),
        ];
        source.callers = Some(callers);
        source.addresses = vec![lookup(1, "app::allocate"), lookup(2, "noise::free")];
        let deallocated = super::super::data::deallocated_allocations(&source);
        let index = Arc::new(FilterIndex::new(
            DecodedSnapshot::default(),
            Some(source.clone()),
            None,
            source.addresses.clone(),
            deallocated,
        ));
        let filtered = index.render(&FilterSpec::parse("crate:app", "crate:noise", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(
            filtered
                .allocations
                .unwrap()
                .hotspots
                .iter()
                .map(|hotspot| (
                    hotspot.allocations,
                    hotspot.allocated_bytes,
                    hotspot.live_allocations,
                    hotspot.live_bytes
                ))
                .collect::<Vec<_>>(),
            [(1, 64, 0, 0)]
        );
        assert_eq!(filtered.memory.unwrap().live_bytes, 777);
        let reset = index.render(&FilterSpec::default());
        assert_eq!(reset.allocations.unwrap(), AllocationSnapshot::from_snapshot(&source));
        assert_eq!(reset.memory.unwrap(), MemorySnapshot::from_snapshot(&source));
    }

    #[test]
    fn spawn_provenance_keeps_complete_task_lifecycle() {
        let events = [EventKind::TaskSpawned, EventKind::TaskPollStarted, EventKind::TaskCompleted]
            .into_iter()
            .enumerate()
            .map(|(sequence, kind)| Event {
                kind,
                payload: EventPayload::Runtime(RuntimeEvent {
                    runtime_id: RuntimeId::from_raw(1).unwrap(),
                    worker_id: (kind != EventKind::TaskSpawned).then(|| WorkerId::from_raw(1).unwrap()),
                    subject_id: 42,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                }),
                ..event(u64::try_from(sequence).unwrap(), (kind == EventKind::TaskSpawned).then_some(1))
            })
            .collect();
        let index = index(events, vec![lookup(1, "app::spawn")]);
        let spawned = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Spawn).unwrap());
        assert_eq!(spawned.filter_summary.events.shown, 3);
        assert_eq!(spawned.runtime.workers[0].tasks[0].state, "Completed");
        let event_stack = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(event_stack.filter_summary.events.shown, 1);
        assert_eq!(event_stack.runtime.workers[0].tasks[0].state, "Spawned");
    }

    #[test]
    fn runtime_metadata_cannot_reintroduce_an_excluded_current_task() {
        use seismograph::recorder::event::BacktraceCapture;
        use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
        use seismograph_runtime::snapshot::{Counters, Runtime, RuntimeState, Task, TaskMetrics, Worker, WorkerState};
        use seismograph_runtime::worker::WorkerRole;

        let source = RuntimeSource {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "executor".into(),
                configured_workers: 1,
                lifecycle_backtraces: BacktraceCapture::Never,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(0),
                retired_at: None,
                counters: Counters::default(),
                workers: vec![Worker {
                    id: WorkerId::from_raw(1).unwrap(),
                    role: WorkerRole::Core,
                    state: WorkerState::Running,
                    processor_index: None,
                    thread_id: Some(ThreadId::new(1)),
                    current_task: Some(TaskId::from_raw(1).unwrap()),
                }],
                tasks: [1, 2]
                    .into_iter()
                    .map(|id| Task {
                        id: TaskId::from_raw(id).unwrap(),
                        parent: None,
                        type_descriptor: TypeDescriptorId::from_raw(1).unwrap(),
                        spawned_at: EventTimestamp::from_ticks(1),
                        last_worker_id: Some(WorkerId::from_raw(1).unwrap()),
                        metrics: TaskMetrics {
                            poll_count: 7,
                            ..TaskMetrics::default()
                        },
                        spawn_backtrace: vec![Address::new(id)],
                    })
                    .collect(),
            }],
            addresses: Vec::new(),
        };
        let addresses = vec![lookup(1, "noise::spawn"), lookup(2, "app::spawn")];
        let decoded = DecodedSnapshot::default();
        let original = RuntimeSnapshot::from_events(&decoded, &addresses, Some(&source));
        let index = Arc::new(FilterIndex::new(decoded, None, Some(source), addresses, HashSet::new()));
        let filtered = index.render(&FilterSpec::parse("crate:app", "crate:noise", false, RuntimeStackMode::Spawn).unwrap());
        let worker = &filtered.runtime.workers[0];
        assert_eq!(
            (
                worker.current_task,
                worker.tasks.iter().map(|task| (task.task_id, task.poll_count)).collect::<Vec<_>>()
            ),
            (None, vec![(2, 7)])
        );
        assert_eq!(
            filtered.filter_summary.tasks,
            FilterCounts {
                total: 2,
                shown: 1,
                unknown: 0
            }
        );
        assert_eq!(index.render(&FilterSpec::default()).runtime, original.runtime);
    }

    #[test]
    fn io_pairs_match_all_captured_frames_without_inventing_pending_operations() {
        use seismograph::recorder::io::{IoEvent, IoOperationId, IoOutcome, IoResourceId, IoResourceKind};
        let events = [EventKind::IoReadStarted, EventKind::IoReadFinished]
            .into_iter()
            .enumerate()
            .map(|(sequence, kind)| Event {
                kind,
                payload: EventPayload::Io(IoEvent {
                    resource_id: IoResourceId::from_raw(1).unwrap(),
                    operation_id: IoOperationId::from_raw(1).unwrap(),
                    resource_kind: IoResourceKind::File,
                    buffer_id: None,
                    requested_bytes: 8,
                    completed_bytes: if kind == EventKind::IoReadFinished { 8 } else { 0 },
                    buffer_len: 8,
                    buffer_span_count: 1,
                    outcome: if kind == EventKind::IoReadFinished {
                        IoOutcome::Success
                    } else {
                        IoOutcome::Pending
                    },
                }),
                ..event(
                    u64::try_from(sequence).unwrap(),
                    Some(if kind == EventKind::IoReadStarted { 1 } else { 2 }),
                )
            })
            .collect();
        let index = index(events, vec![lookup(1, "app::read"), lookup(2, "noise::completion")]);
        let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
        assert_eq!((filtered.filter_summary.events.shown, filtered.io.resources[0].completed), (2, 1));
        assert_eq!(filtered.io.resources[0].operations[0].outcome, IoOutcome::Success);
        let excluded = index.render(&FilterSpec::parse("crate:app", "crate:noise", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(excluded.filter_summary.events.shown, 0);
        assert!(excluded.io.resources.is_empty());
    }

    #[test]
    fn identical_stacks_are_interned_and_retained_events_have_no_stack_allocations() {
        let index = index(
            (1..=100).map(|sequence| event(sequence, Some(1))).collect(),
            vec![lookup(1, "app::work")],
        );
        assert_eq!((index.events.len(), index.stacks.len()), (100, 2));
        assert!(index.events.iter().all(|event| event.stack == StackId(1)));
    }
}
