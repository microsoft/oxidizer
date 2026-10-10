// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use seismograph::recorder::event::{Address, Event, EventKind, EventPayload, EventSequence, EventTimestamp};
use seismograph::recorder::thread::ThreadId;
use seismograph::snapshot::DecodedSnapshot;
use seismograph_rallocator::callers::{AddressLookup, Event as AllocationEvent, EventKind as AllocationEventKind};
use seismograph_runtime::snapshot::Snapshot as RuntimeSource;

use super::data::{
    AllocationSnapshot, CapturedSnapshot, MemorySnapshot, RuntimeSnapshot, RuntimeTaskSummary, RuntimeWorkerSummary, runtime_task_id,
};
use super::filter::{FilterSpec, Match, RuntimeStackMode, StackProvenance};
use super::runtime_timeline::TimeWindow;
use crate::allocator_view::Snapshot as AllocatorSource;

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
    runtime_windows: BTreeMap<u64, TimeWindow>,
    task_stacks: HashMap<(u64, u64), StackId>,
    source_task_stacks: HashMap<(u64, u64), StackId>,
    io_stacks: HashMap<u64, StackId>,
    task_events: super::task_events::TaskEventsSnapshot,
}

impl FilterIndex {
    #[cfg(test)]
    pub(super) fn new(
        decoded: DecodedSnapshot,
        allocator: Option<AllocatorSource>,
        runtime: Option<RuntimeSource>,
        addresses: Vec<AddressLookup>,
        deallocated: HashSet<(u64, u64)>,
    ) -> Self {
        let task_events = super::task_events::TaskEventsSnapshot::from_events(&decoded.events, &addresses, runtime.as_ref());
        Self::with_task_events(decoded, allocator, runtime, addresses, deallocated, task_events)
    }

    pub(super) fn with_task_events(
        mut decoded: DecodedSnapshot,
        mut allocator: Option<AllocatorSource>,
        mut runtime: Option<RuntimeSource>,
        addresses: Vec<AddressLookup>,
        deallocated: HashSet<(u64, u64)>,
        task_events: super::task_events::TaskEventsSnapshot,
    ) -> Self {
        let runtime_windows = super::runtime::display_windows(&decoded.events, runtime.as_ref());
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
            runtime_windows,
            task_stacks,
            source_task_stacks,
            io_stacks,
            task_events,
        }
    }

    pub(super) fn unfiltered_summary(&self) -> FilterSummary {
        FilterSummary {
            active: false,
            events: FilterCounts::unfiltered(self.events.len()),
            allocations: FilterCounts::unfiltered(
                self.allocations
                    .iter()
                    .filter(|allocation| allocation.event.kind == AllocationEventKind::Allocated)
                    .count(),
            ),
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
        let task_events = if filter.is_active() {
            self.task_events.filtered(
                &decoded
                    .events
                    .events
                    .iter()
                    .map(|event| (event.thread_id.get(), event.sequence.get()))
                    .collect(),
            )
        } else {
            self.task_events.clone()
        };
        visible_tasks.extend(task_events.tasks.keys().copied());
        summary.tasks.shown = u64::try_from(visible_tasks.len()).unwrap_or(u64::MAX);
        if filter.is_active() {
            let visible_threads = decoded.events.events.iter().map(|event| event.thread_id).collect::<HashSet<_>>();
            decoded.events.threads.retain(|thread| visible_threads.contains(&thread.thread_id));
        }
        let source = self.runtime_source(filter, &visible_tasks, &decoded.events.events);
        let mut runtime =
            RuntimeSnapshot::from_events_with_attribution(&decoded, &self.addresses, source.as_ref(), task_events, &mut |_| {});
        self.retain_inferred_task_rows(&mut runtime);
        runtime.runtime.set_windows(&self.runtime_windows);
        super::snapshot::release_stacks(&mut decoded.events.events, |event| drop(std::mem::take(&mut event.call_stack)));
        drop(decoded);
        drop(source);

        let mut allocation_events = self.filtered_allocations(filter, &matches, &mut summary);
        let memory = self
            .allocator
            .as_ref()
            .filter(|source| source.allocator_state_available)
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
            native: self.allocator.as_ref().and_then(|source| source.native.clone()),
            memory,
            allocations,
            heap_error: super::snapshot::heap_error(self.allocator.as_ref()),
            primitives: runtime.primitives,
            runtime: runtime.runtime,
            io: runtime.io,
            cache: runtime.cache,
            threads: runtime.threads,
            task_events: runtime.task_events,
            captured_at: None,
            captured_instant: None,
            filter_index: Some(Arc::clone(self)),
            filter_summary: summary,
        })
    }

    fn retain_inferred_task_rows(&self, snapshot: &mut RuntimeSnapshot) {
        let present = snapshot
            .runtime
            .workers
            .iter()
            .flat_map(|worker| worker.tasks.iter().map(|task| (task.runtime_id, task.task_id)))
            .collect::<HashSet<_>>();
        let names = self
            .runtime
            .iter()
            .flat_map(|source| &source.runtimes)
            .map(|runtime| (runtime.id.get(), runtime.name.as_str()))
            .collect::<HashMap<_, _>>();
        let workers = &mut snapshot.runtime.workers;
        let mut unbound = workers
            .iter()
            .enumerate()
            .filter_map(|(index, worker)| worker.worker_id.is_none().then_some((worker.runtime_id, index)))
            .collect::<BTreeMap<_, _>>();
        for &(runtime_id, task_id) in snapshot.task_events.tasks.keys().filter(|task| !present.contains(*task)) {
            let index = *unbound.entry(runtime_id).or_insert_with(|| {
                workers.push(RuntimeWorkerSummary {
                    runtime_id,
                    runtime_name: names
                        .get(&runtime_id)
                        .map_or_else(|| format!("runtime #{runtime_id}"), |name| (*name).into()),
                    role: "Unbound".into(),
                    state: "Unknown".into(),
                    ..RuntimeWorkerSummary::default()
                });
                workers.len() - 1
            });
            // The actor identity survives filtering, not its hidden poll samples,
            // spawn stack, or worker associations. Keep it navigable without
            // turning attribution context into visible runtime-event evidence.
            workers[index].tasks.push(RuntimeTaskSummary {
                task_id,
                runtime_id,
                state: "Unknown".into(),
                ..RuntimeTaskSummary::default()
            });
        }
        for index in unbound.into_values() {
            workers[index].tasks.sort_unstable_by_key(|task| task.task_id);
        }
        workers.sort_unstable_by_key(|worker| (worker.runtime_id, worker.worker_id));
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

    fn filtered_allocations(&self, filter: &FilterSpec, matches: &[Match], summary: &mut FilterSummary) -> Vec<AllocationEvent> {
        self.allocations
            .iter()
            .filter_map(|allocation| {
                let shown = if allocation.event.kind == AllocationEventKind::Allocated {
                    summary.allocations.observe(filter, matches[allocation.stack.0])
                } else {
                    filter.accepts(matches[allocation.stack.0])
                };
                if !shown {
                    return None;
                }
                let mut event = allocation.event.clone();
                event.call_stack = self.stacks[allocation.stack.0].addresses.to_vec();
                Some(event)
            })
            .collect()
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
                    if let Some(worker) = task.activity.and_then(|activity| activity.poll_worker_id) {
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
    use seismograph::recorder::event::{EventClock, Events, ObjectId};
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
    use seismograph::recorder::thread::ThreadLog;
    use seismograph::recorder::{RecordingPolicies, RecordingPolicy};
    use seismograph_rallocator::callers::{AddressLookupFields, Callers};

    use super::*;
    use crate::allocator_view::Version;

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
            filtered.filter_summary,
            FilterSummary {
                active: true,
                events: FilterCounts {
                    total: 3,
                    shown: 1,
                    unknown: 1
                },
                allocations: FilterCounts::default(),
                tasks: FilterCounts::default(),
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
        assert_eq!(
            index.unfiltered_summary(),
            FilterSummary {
                active: false,
                events: FilterCounts::default(),
                allocations: FilterCounts {
                    total: 2,
                    shown: 2,
                    unknown: 0,
                },
                tasks: FilterCounts::default(),
            }
        );
        let filtered = index.render(&FilterSpec::parse("crate:app", "crate:noise", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(
            filtered.filter_summary.allocations,
            FilterCounts {
                total: 2,
                shown: 1,
                unknown: 0,
            }
        );
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
        let events = [
            EventKind::TaskSpawned,
            EventKind::TaskReady,
            EventKind::TaskPollStarted,
            EventKind::TaskCompleted,
        ]
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
        assert_eq!(spawned.filter_summary.events.shown, 4);
        assert_eq!(spawned.runtime.workers[0].tasks[0].state, "Completed");
        let event_stack = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(event_stack.filter_summary.events.shown, 1);
        assert_eq!(event_stack.runtime.workers[0].tasks[0].state, "Unknown");
    }

    #[test]
    fn repeated_spawn_events_keep_the_first_spawn_provenance() {
        let events = [(1, 1), (2, 2)]
            .into_iter()
            .map(|(sequence, address)| Event {
                kind: EventKind::TaskSpawned,
                payload: EventPayload::Runtime(RuntimeEvent {
                    runtime_id: RuntimeId::from_raw(1).unwrap(),
                    worker_id: None,
                    subject_id: 42,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                }),
                ..event(sequence, Some(address))
            })
            .collect();
        let index = index(events, vec![lookup(1, "app::spawn"), lookup(2, "noise::spawn")]);
        let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Spawn).unwrap());
        assert_eq!((filtered.filter_summary.events.shown, filtered.filter_summary.tasks.shown), (2, 1));
    }

    #[test]
    fn filtered_runtime_keeps_workers_with_either_task_or_event_evidence() {
        use seismograph::recorder::event::BacktraceCapture;
        use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
        use seismograph_runtime::snapshot::{Counters, Runtime, RuntimeState, Task, TaskMetrics, Worker, WorkerState};
        use seismograph_runtime::worker::WorkerRole;

        let source = RuntimeSource {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "executor".into(),
                configured_workers: 2,
                lifecycle_backtraces: BacktraceCapture::Never,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(1),
                retired_at: None,
                counters: Counters::default(),
                workers: [Some(TaskId::from_raw(1).unwrap()), None]
                    .into_iter()
                    .enumerate()
                    .map(|(index, current_task)| Worker {
                        id: WorkerId::from_raw(u64::try_from(index).unwrap() + 1).unwrap(),
                        role: WorkerRole::Core,
                        state: WorkerState::Running,
                        processor_index: None,
                        thread_id: None,
                        current_task,
                    })
                    .collect(),
                tasks: vec![Task {
                    id: TaskId::from_raw(1).unwrap(),
                    parent: None,
                    type_descriptor: TypeDescriptorId::from_raw(1).unwrap(),
                    future_size_bytes: None,
                    spawned_at: EventTimestamp::from_ticks(1),
                    last_worker_id: None,
                    activity: None,
                    metrics: TaskMetrics::default(),
                    spawn_backtrace: Vec::new(),
                }],
            }],
            addresses: Vec::new(),
        };
        let index = FilterIndex::new(DecodedSnapshot::default(), None, Some(source), Vec::new(), HashSet::new());
        let worker_event = Event {
            kind: EventKind::WorkerParked,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: Some(WorkerId::from_raw(2).unwrap()),
                subject_id: 0,
                related_id: 0,
                value_0: 0,
                value_1: 0,
            }),
            ..event(1, None)
        };
        let filtered = index
            .runtime_source(
                &FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap(),
                &HashSet::from([(1, 1)]),
                &[worker_event],
            )
            .unwrap();
        assert_eq!(
            filtered.runtimes[0]
                .workers
                .iter()
                .map(|worker| (worker.id.get(), worker.current_task.map(TaskId::get)))
                .collect::<Vec<_>>(),
            [(1, Some(1)), (2, None)]
        );
    }

    #[test]
    fn inferred_task_rows_preserve_identity_when_runtime_boundaries_are_filtered() {
        let runtime = |sequence, timestamp, kind, duration| Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(7).unwrap(),
                worker_id: Some(WorkerId::from_raw(3).unwrap()),
                subject_id: 42,
                related_id: 0,
                value_0: duration,
                value_1: 0,
            }),
            call_stack: vec![Address::new(2)],
        };
        let operation = Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(2),
            timestamp: EventTimestamp::from_ticks(20),
            kind: EventKind::MutexAccess,
            payload: EventPayload::Object(ObjectId::new(9)),
            call_stack: vec![Address::new(1)],
        };
        let events = Events {
            clock: EventClock::ProcessMonotonic,
            total_events: 3,
            recording: RecordingPolicies {
                runtime_tasks: RecordingPolicy::all(true),
                general_events: RecordingPolicy::all(true),
                ..RecordingPolicies::default()
            },
            threads: vec![ThreadLog {
                thread_id: ThreadId::new(1),
                total_events: 3,
                ..ThreadLog::default()
            }],
            events: vec![
                runtime(1, 10, EventKind::TaskPollStarted, 0),
                operation,
                runtime(3, 30, EventKind::TaskPollFinished, 20),
            ],
            ..Events::default()
        };
        let index = index(events.events.clone(), vec![lookup(1, "app::work"), lookup(2, "noise::poll")]);
        let mut decoded = index.header.clone();
        decoded.events = events;
        let index = Arc::new(FilterIndex::new(
            decoded,
            None,
            None,
            vec![lookup(1, "app::work"), lookup(2, "noise::poll")],
            HashSet::new(),
        ));
        let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
        assert_eq!(
            filtered
                .runtime
                .workers
                .iter()
                .map(|worker| {
                    (
                        worker.runtime_id,
                        worker.runtime_name.as_str(),
                        worker.role.as_str(),
                        worker.state.as_str(),
                        worker.tasks[0].runtime_id,
                        worker.tasks[0].task_id,
                        worker.tasks[0].state.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            [(7, "runtime #7", "Unbound", "Unknown", 7, 42, "Unknown")]
        );
    }

    #[test]
    fn filtered_runtime_execution_uses_the_original_capture_window() {
        let events = [(100, 20, 1), (200, 10, 2)]
            .into_iter()
            .map(|(timestamp, duration, address)| Event {
                kind: EventKind::TaskPollFinished,
                payload: EventPayload::Runtime(RuntimeEvent {
                    runtime_id: RuntimeId::from_raw(1).unwrap(),
                    worker_id: Some(WorkerId::from_raw(1).unwrap()),
                    subject_id: 42,
                    related_id: 0,
                    value_0: duration,
                    value_1: 0,
                }),
                ..event(timestamp, Some(address))
            })
            .collect();
        let index = index(events, vec![lookup(1, "app::poll"), lookup(2, "noise::poll")]);
        let original = index.render(&FilterSpec::default());
        let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
        let worker = &filtered.runtime.workers[0];
        assert_eq!(
            (worker.window, worker.metrics.poll_count, worker.tasks[0].metrics.poll_count),
            (original.runtime.workers[0].window, 1, 1)
        );
        assert_eq!(worker.window, Some(TimeWindow { start: 80, end: 200 }));
        for metrics in [&worker.metrics, &worker.tasks[0].metrics] {
            assert!((metrics.executing_fraction.unwrap() - 1.0 / 6.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "the fixture must keep one coherent runtime metadata graph")]
    fn runtime_metadata_cannot_reintroduce_an_excluded_current_task() {
        use seismograph::recorder::event::BacktraceCapture;
        use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
        use seismograph_runtime::snapshot::{
            Counters, Runtime, RuntimeState, Task, TaskActivity, TaskActivityState, TaskMetrics, Worker, WorkerState,
        };
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
                        future_size_bytes: None,
                        spawned_at: EventTimestamp::from_ticks(1),
                        last_worker_id: Some(WorkerId::from_raw(1).unwrap()),
                        activity: (id == 2).then_some(TaskActivity {
                            poll_worker_id: Some(WorkerId::from_raw(1).unwrap()),
                            observed_at: EventTimestamp::from_ticks(100),
                            state: TaskActivityState::Unknown,
                            ready_since: None,
                            poll_started_at: None,
                            queued_since: None,
                        }),
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
        let mut runtime_without_worker = event(2, Some(2));
        runtime_without_worker.kind = EventKind::TaskReady;
        runtime_without_worker.payload = EventPayload::Runtime(RuntimeEvent {
            runtime_id: RuntimeId::from_raw(1).unwrap(),
            worker_id: None,
            subject_id: 2,
            related_id: 0,
            value_0: 0,
            value_1: 0,
        });
        let decoded = DecodedSnapshot {
            events: Events {
                total_events: 2,
                events: vec![event(1, Some(1)), runtime_without_worker],
                ..Events::default()
            },
            ..DecodedSnapshot::default()
        };
        let original = RuntimeSnapshot::from_events(&decoded, &addresses, Some(&source));
        let index = Arc::new(FilterIndex::new(decoded, None, Some(source), addresses, HashSet::new()));
        let filtered = index.render(&FilterSpec::parse("crate:app", "crate:noise", false, RuntimeStackMode::Spawn).unwrap());
        let worker = &filtered.runtime.workers[0];
        assert_eq!(
            (
                worker.current_task,
                worker
                    .tasks
                    .iter()
                    .map(|task| (task.task_id, task.metrics.poll_count))
                    .collect::<Vec<_>>()
            ),
            (None, vec![(2, 0)])
        );
        assert_eq!(
            filtered.filter_summary.tasks,
            FilterCounts {
                total: 2,
                shown: 1,
                unknown: 0
            }
        );
        assert_eq!(
            (
                worker.tasks[0].activity.state.as_str(),
                worker.tasks[0].activity.running_for,
                worker.tasks[0].activity.ready_for
            ),
            ("Unknown", None, None)
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
