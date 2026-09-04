// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

pub(super) struct CapturedSnapshot {
    pub(super) memory: Option<MemorySnapshot>,
    pub(super) allocations: Option<AllocationSnapshot>,
    pub(super) heap_error: Option<String>,
    pub(super) primitives: PrimitiveSnapshot,
    pub(super) runtime: RuntimeMonitorSnapshot,
    pub(super) threads: ThreadSnapshot,
    pub(super) captured_at: SystemTime,
    pub(super) captured_instant: Instant,
}

pub(super) struct RuntimeSnapshot {
    pub(super) primitives: PrimitiveSnapshot,
    pub(super) runtime: RuntimeMonitorSnapshot,
    pub(super) threads: ThreadSnapshot,
}

impl RuntimeSnapshot {
    pub(super) fn from_events(
        decoded: &seismograph::snapshot::DecodedSnapshot,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
        runtime_source: Option<&seismograph_runtime::snapshot::Snapshot>,
    ) -> Self {
        Self {
            primitives: PrimitiveSnapshot::from_events(
                decoded.events.total_events,
                decoded.events.lost_events,
                &decoded.events.events,
                addresses,
            ),
            runtime: RuntimeMonitorSnapshot::from_events(&decoded.events, runtime_source, addresses),
            threads: ThreadSnapshot::from_events(&decoded.events, addresses),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RuntimeMonitorSnapshot {
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) workers: Vec<RuntimeWorkerSummary>,
}

impl RuntimeMonitorSnapshot {
    #[expect(
        clippy::too_many_lines,
        reason = "runtime events are decoded in one ordered pass so cross-event timing state remains explicit"
    )]
    fn from_events(
        events: &seismograph::recorder::event::Events,
        source: Option<&seismograph_runtime::snapshot::Snapshot>,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
    ) -> Self {
        use seismograph::recorder::event::EventKind;

        #[derive(Default)]
        struct WorkerBuilder {
            runtime_name: String,
            role: String,
            state: String,
            thread_id: Option<u64>,
            current_task: Option<u64>,
            first_timestamp: Option<u64>,
            last_timestamp: Option<u64>,
            poll_count: u64,
            poll_nanos: u64,
            max_poll_nanos: u64,
            task_ids: HashSet<u64>,
        }

        let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
        let mut workers = BTreeMap::<(u64, u64), WorkerBuilder>::new();
        let mut tasks = BTreeMap::<u64, RuntimeTaskBuilder>::new();
        if let Some(source) = source {
            for runtime in &source.runtimes {
                for source_task in &runtime.tasks {
                    let task = tasks.entry(source_task.id.get()).or_default();
                    task.runtime_id = runtime.id.get();
                    task.parent_id = source_task.parent.map(seismograph::recorder::runtime::TaskId::get);
                    task.type_descriptor_id = Some(source_task.type_descriptor.get());
                    task.state = "Pending".into();
                    task.spawned_at = (source_task.spawned_at.ticks() != 0).then_some(source_task.spawned_at.ticks());
                    task.metric_scope = RuntimeTaskMetricScope::Lifetime;
                    task.poll_count = source_task.metrics.poll_count;
                    task.poll_nanos = source_task.metrics.poll_duration_nanos;
                    task.max_poll_nanos = source_task.metrics.max_poll_duration_nanos;
                    task.resume_count = source_task.metrics.resume_count;
                    task.resume_nanos = source_task.metrics.resume_duration_nanos;
                    task.max_resume_nanos = source_task.metrics.max_resume_duration_nanos;
                    task.ready_wait_count = source_task.metrics.ready_wait_count;
                    task.ready_wait_nanos = source_task.metrics.ready_wait_duration_nanos;
                    task.max_ready_wait_nanos = source_task.metrics.max_ready_wait_duration_nanos;
                    let stack = source_task
                        .spawn_backtrace
                        .iter()
                        .copied()
                        .map(seismograph::recorder::event::Address::get)
                        .collect::<Vec<_>>();
                    task.spawn_stack = primitive_stack(&stack, &lookups, AllocationStackFilter::All);
                }
                for worker in &runtime.workers {
                    let current_task = worker.current_task.map(seismograph::recorder::runtime::TaskId::get);
                    let mut task_ids = HashSet::new();
                    if let Some(task_id) = current_task {
                        task_ids.insert(task_id);
                        let task = tasks.entry(task_id).or_default();
                        task.runtime_id = runtime.id.get();
                        task.state = "Running".into();
                        task.worker_ids.insert(worker.id.get());
                    }
                    workers.insert(
                        (runtime.id.get(), worker.id.get()),
                        WorkerBuilder {
                            runtime_name: runtime.name.clone(),
                            role: format!("{:?}", worker.role),
                            state: format!("{:?}", worker.state),
                            thread_id: worker.thread_id.map(seismograph::recorder::thread::ThreadId::get),
                            current_task,
                            task_ids,
                            ..WorkerBuilder::default()
                        },
                    );
                }

                for source_task in &runtime.tasks {
                    let Some(worker_id) = source_task.last_worker_id else {
                        continue;
                    };
                    let worker_id = worker_id.get();
                    if let Some(worker) = workers.get_mut(&(runtime.id.get(), worker_id)) {
                        worker.task_ids.insert(source_task.id.get());
                        tasks.entry(source_task.id.get()).or_default().worker_ids.insert(worker_id);
                    }
                }
            }
        }

        for event in &events.events {
            let Some(runtime) = event.runtime() else {
                continue;
            };
            let runtime_id = runtime.runtime_id.get();
            let timestamp = event.timestamp.ticks();
            if let Some(worker_id) = runtime.worker_id.map(seismograph::recorder::runtime::WorkerId::get) {
                let worker = workers.entry((runtime_id, worker_id)).or_default();
                worker.first_timestamp = Some(worker.first_timestamp.map_or(timestamp, |first| first.min(timestamp)));
                worker.last_timestamp = Some(worker.last_timestamp.map_or(timestamp, |last| last.max(timestamp)));
                let task_id = runtime_task_id(event.kind, runtime.subject_id, runtime.related_id);
                if let Some(task_id) = task_id {
                    worker.task_ids.insert(task_id);
                    tasks.entry(task_id).or_default().worker_ids.insert(worker_id);
                }
                if event.kind == EventKind::TaskPollFinished {
                    worker.poll_count = worker.poll_count.saturating_add(1);
                    worker.poll_nanos = worker.poll_nanos.saturating_add(runtime.value_0);
                    worker.max_poll_nanos = worker.max_poll_nanos.max(runtime.value_0);
                }
            }

            if event.kind == EventKind::TaskSpawned {
                let task = tasks.entry(runtime.subject_id).or_default();
                record_task_spawn(task, runtime_id, timestamp, event, &lookups);
            }

            let Some(task_id) = runtime_task_id(event.kind, runtime.subject_id, runtime.related_id) else {
                continue;
            };
            let task = tasks.entry(task_id).or_default();
            task.runtime_id = runtime_id;
            match event.kind {
                EventKind::TaskEnqueued => task.enqueue_count = task.enqueue_count.saturating_add(1),
                EventKind::TaskMaterialized => {
                    task.materialization_count = task.materialization_count.saturating_add(1);
                    if task.metric_scope == RuntimeTaskMetricScope::RetainedWindow {
                        task.state = "Materialized".into();
                    }
                }
                EventKind::TaskPollStarted if task.metric_scope == RuntimeTaskMetricScope::RetainedWindow => {
                    if let Some(previous_poll_finished) = task.last_poll_finished_at.take() {
                        let resume_nanos = timestamp.saturating_sub(previous_poll_finished);
                        task.resume_count = task.resume_count.saturating_add(1);
                        task.resume_nanos = task.resume_nanos.saturating_add(resume_nanos);
                        task.max_resume_nanos = task.max_resume_nanos.max(resume_nanos);
                    }
                    if runtime.value_1 != 0 {
                        task.ready_wait_count = task.ready_wait_count.saturating_add(1);
                        task.ready_wait_nanos = task.ready_wait_nanos.saturating_add(runtime.value_0);
                        task.max_ready_wait_nanos = task.max_ready_wait_nanos.max(runtime.value_0);
                    }
                    task.state = "Running".into();
                }
                EventKind::TaskPollFinished if task.metric_scope == RuntimeTaskMetricScope::RetainedWindow => {
                    task.poll_count = task.poll_count.saturating_add(1);
                    task.poll_nanos = task.poll_nanos.saturating_add(runtime.value_0);
                    task.max_poll_nanos = task.max_poll_nanos.max(runtime.value_0);
                    task.last_poll_finished_at = Some(timestamp);
                    task.state = "Pending".into();
                }
                EventKind::TaskCompleted if task.metric_scope == RuntimeTaskMetricScope::RetainedWindow => {
                    task.state = "Completed".into();
                    task.completed_at = Some(timestamp);
                }
                EventKind::TaskCanceled if task.metric_scope == RuntimeTaskMetricScope::RetainedWindow => {
                    task.state = "Canceled".into();
                    task.completed_at = Some(timestamp);
                }
                EventKind::TaskPanicked if task.metric_scope == RuntimeTaskMetricScope::RetainedWindow => {
                    task.state = "Panicked".into();
                    task.completed_at = Some(timestamp);
                }
                EventKind::TransferStarted | EventKind::InstanceRelocated | EventKind::TransferFinished => {
                    task.transfer_count = task.transfer_count.saturating_add(1);
                }
                _ => {}
            }
        }

        let mut summaries = workers
            .into_iter()
            .map(|((runtime_id, worker_id), worker)| {
                let span_nanos = worker
                    .first_timestamp
                    .zip(worker.last_timestamp)
                    .map_or(0, |(first, last)| last.saturating_sub(first));
                let mut worker_tasks = worker
                    .task_ids
                    .iter()
                    .filter_map(|task_id| tasks.get(task_id).map(|task| RuntimeTaskSummary::from_builder(*task_id, task)))
                    .collect::<Vec<_>>();
                worker_tasks.sort_unstable_by_key(|task| task.task_id);
                RuntimeWorkerSummary {
                    runtime_id,
                    runtime_name: worker.runtime_name,
                    worker_id,
                    role: worker.role,
                    state: worker.state,
                    thread_id: worker.thread_id,
                    current_task: worker.current_task,
                    average_running_tasks: if span_nanos == 0 {
                        0.0
                    } else {
                        Duration::from_nanos(worker.poll_nanos).as_secs_f64() / Duration::from_nanos(span_nanos).as_secs_f64()
                    },
                    poll_count: worker.poll_count,
                    average_poll_nanos: worker.poll_nanos.checked_div(worker.poll_count).unwrap_or_default(),
                    max_poll_nanos: worker.max_poll_nanos,
                    tasks: worker_tasks,
                }
            })
            .collect::<Vec<_>>();
        summaries.sort_unstable_by_key(|worker| (worker.runtime_id, worker.worker_id));
        Self {
            total_events: events.total_events,
            retained_events: u64::try_from(events.events.len()).unwrap_or(u64::MAX),
            lost_events: events.lost_events,
            workers: summaries,
        }
    }
}

fn runtime_task_id(kind: seismograph::recorder::event::EventKind, subject_id: u64, related_id: u64) -> Option<u64> {
    use seismograph::recorder::event::EventKind;
    match kind {
        EventKind::TaskSpawned
        | EventKind::TaskEnqueued
        | EventKind::TaskMaterialized
        | EventKind::TaskPollStarted
        | EventKind::TaskPollFinished
        | EventKind::TaskCompleted
        | EventKind::TaskCanceled
        | EventKind::TaskPanicked => (subject_id != 0).then_some(subject_id),
        EventKind::TransferStarted | EventKind::InstanceRelocated | EventKind::TransferFinished => (related_id != 0).then_some(related_id),
        _ => None,
    }
}

fn record_task_spawn(
    task: &mut RuntimeTaskBuilder,
    runtime_id: u64,
    timestamp: u64,
    event: &seismograph::recorder::event::Event,
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
) {
    if task.metric_scope != RuntimeTaskMetricScope::RetainedWindow {
        return;
    }
    let runtime = event.runtime().expect("called only for task-spawn runtime events");
    task.runtime_id = runtime_id;
    task.parent_id = (runtime.related_id != 0).then_some(runtime.related_id);
    task.type_descriptor_id = (runtime.value_0 != 0).then_some(runtime.value_0);
    task.state = "Spawned".into();
    task.spawned_at = Some(timestamp);
    let stack = event.call_stack.iter().map(|address| address.get()).collect::<Vec<_>>();
    task.spawn_stack = primitive_stack(&stack, lookups, AllocationStackFilter::All);
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RuntimeWorkerSummary {
    pub(super) runtime_id: u64,
    pub(super) runtime_name: String,
    pub(super) worker_id: u64,
    pub(super) role: String,
    pub(super) state: String,
    pub(super) thread_id: Option<u64>,
    pub(super) current_task: Option<u64>,
    pub(super) average_running_tasks: f64,
    pub(super) poll_count: u64,
    pub(super) average_poll_nanos: u64,
    pub(super) max_poll_nanos: u64,
    pub(super) tasks: Vec<RuntimeTaskSummary>,
}

impl RuntimeWorkerSummary {
    pub(super) fn sorted_tasks(&self, sort: RuntimeTaskSort, descending: bool) -> Vec<&RuntimeTaskSummary> {
        let mut tasks = self.tasks.iter().collect::<Vec<_>>();
        tasks.sort_unstable_by(|left, right| {
            let ordering = match sort {
                RuntimeTaskSort::Task => left.task_id.cmp(&right.task_id),
                RuntimeTaskSort::Polls => left.poll_count.cmp(&right.poll_count),
                RuntimeTaskSort::AveragePoll => left.average_poll_nanos.cmp(&right.average_poll_nanos),
                RuntimeTaskSort::MaximumPoll => left.max_poll_nanos.cmp(&right.max_poll_nanos),
                RuntimeTaskSort::AverageResume => left.average_resume_nanos.cmp(&right.average_resume_nanos),
                RuntimeTaskSort::MaximumResume => left.max_resume_nanos.cmp(&right.max_resume_nanos),
                RuntimeTaskSort::AverageReadyWait => left.average_ready_wait_nanos.cmp(&right.average_ready_wait_nanos),
                RuntimeTaskSort::MaximumReadyWait => left.max_ready_wait_nanos.cmp(&right.max_ready_wait_nanos),
            };
            let ordering = if descending { ordering.reverse() } else { ordering };
            ordering.then_with(|| left.task_id.cmp(&right.task_id))
        });
        tasks
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeTaskSort {
    Task,
    Polls,
    AveragePoll,
    MaximumPoll,
    AverageResume,
    MaximumResume,
    AverageReadyWait,
    MaximumReadyWait,
}

impl RuntimeTaskSort {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Task => Self::Polls,
            Self::Polls => Self::AveragePoll,
            Self::AveragePoll => Self::MaximumPoll,
            Self::MaximumPoll => Self::AverageResume,
            Self::AverageResume => Self::MaximumResume,
            Self::MaximumResume => Self::AverageReadyWait,
            Self::AverageReadyWait => Self::MaximumReadyWait,
            Self::MaximumReadyWait => Self::Task,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Task => Self::MaximumReadyWait,
            Self::Polls => Self::Task,
            Self::AveragePoll => Self::Polls,
            Self::MaximumPoll => Self::AveragePoll,
            Self::AverageResume => Self::MaximumPoll,
            Self::MaximumResume => Self::AverageResume,
            Self::AverageReadyWait => Self::MaximumResume,
            Self::MaximumReadyWait => Self::AverageReadyWait,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Polls => "polls",
            Self::AveragePoll => "average poll",
            Self::MaximumPoll => "maximum poll",
            Self::AverageResume => "average resume",
            Self::MaximumResume => "maximum resume",
            Self::AverageReadyWait => "average stall",
            Self::MaximumReadyWait => "maximum stall",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RuntimeTaskSummary {
    pub(super) task_id: u64,
    pub(super) runtime_id: u64,
    pub(super) parent_id: Option<u64>,
    pub(super) type_descriptor_id: Option<u64>,
    pub(super) metric_scope: RuntimeTaskMetricScope,
    pub(super) state: String,
    pub(super) spawned_at: Option<u64>,
    pub(super) completed_at: Option<u64>,
    pub(super) poll_count: u64,
    pub(super) poll_nanos: u64,
    pub(super) average_poll_nanos: u64,
    pub(super) max_poll_nanos: u64,
    pub(super) resume_count: u64,
    pub(super) average_resume_nanos: u64,
    pub(super) max_resume_nanos: u64,
    pub(super) ready_wait_count: u64,
    pub(super) ready_wait_nanos: u64,
    pub(super) average_ready_wait_nanos: u64,
    pub(super) max_ready_wait_nanos: u64,
    pub(super) enqueue_count: u64,
    pub(super) materialization_count: u64,
    pub(super) transfer_count: u64,
    pub(super) worker_ids: Vec<u64>,
    pub(super) spawn_stack: Vec<String>,
}

impl RuntimeTaskSummary {
    fn from_builder(task_id: u64, task: &RuntimeTaskBuilder) -> Self {
        Self {
            task_id,
            runtime_id: task.runtime_id,
            parent_id: task.parent_id,
            type_descriptor_id: task.type_descriptor_id,
            metric_scope: task.metric_scope,
            state: task.state.clone(),
            spawned_at: task.spawned_at,
            completed_at: task.completed_at,
            poll_count: task.poll_count,
            poll_nanos: task.poll_nanos,
            average_poll_nanos: task.poll_nanos.checked_div(task.poll_count).unwrap_or_default(),
            max_poll_nanos: task.max_poll_nanos,
            resume_count: task.resume_count,
            average_resume_nanos: task.resume_nanos.checked_div(task.resume_count).unwrap_or_default(),
            max_resume_nanos: task.max_resume_nanos,
            ready_wait_count: task.ready_wait_count,
            ready_wait_nanos: task.ready_wait_nanos,
            average_ready_wait_nanos: task.ready_wait_nanos.checked_div(task.ready_wait_count).unwrap_or_default(),
            max_ready_wait_nanos: task.max_ready_wait_nanos,
            enqueue_count: task.enqueue_count,
            materialization_count: task.materialization_count,
            transfer_count: task.transfer_count,
            worker_ids: task.worker_ids.iter().copied().collect(),
            spawn_stack: task.spawn_stack.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum RuntimeTaskMetricScope {
    Lifetime,
    #[default]
    RetainedWindow,
}

impl RuntimeTaskMetricScope {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Lifetime => "lifetime",
            Self::RetainedWindow => "retained window",
        }
    }
}

#[derive(Default)]
struct RuntimeTaskBuilder {
    runtime_id: u64,
    parent_id: Option<u64>,
    type_descriptor_id: Option<u64>,
    metric_scope: RuntimeTaskMetricScope,
    state: String,
    spawned_at: Option<u64>,
    completed_at: Option<u64>,
    poll_count: u64,
    poll_nanos: u64,
    max_poll_nanos: u64,
    last_poll_finished_at: Option<u64>,
    resume_count: u64,
    resume_nanos: u64,
    max_resume_nanos: u64,
    ready_wait_count: u64,
    ready_wait_nanos: u64,
    max_ready_wait_nanos: u64,
    enqueue_count: u64,
    materialization_count: u64,
    transfer_count: u64,
    worker_ids: HashSet<u64>,
    spawn_stack: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AllocationSnapshot {
    pub(super) thread_count: u64,
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) hotspots: Vec<AllocationHotspot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AllocationHotspot {
    pub(super) allocations: u64,
    pub(super) allocated_bytes: u64,
    pub(super) live_allocations: u64,
    pub(super) live_bytes: u64,
    application_stack: Vec<String>,
    complete_stack: Vec<String>,
}

impl AllocationHotspot {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.application_stack,
            AllocationStackFilter::All => &self.complete_stack,
        }
    }

    pub(super) fn location(&self, filter: AllocationStackFilter) -> &str {
        self.stack(filter).first().map_or("Backtraces disabled", String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AllocationStackFilter {
    Application,
    All,
}

impl AllocationStackFilter {
    pub(super) const fn toggle(self) -> Self {
        match self {
            Self::Application => Self::All,
            Self::All => Self::Application,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveSnapshot {
    pub(super) total_events: u64,
    pub(super) lost_events: u64,
    pub(super) groups: Vec<PrimitiveGroup>,
}

impl PrimitiveSnapshot {
    fn from_events(
        total_events: u64,
        lost_events: u64,
        events: &[seismograph::recorder::event::Event],
        addresses: &[seismograph_rallocator::callers::AddressLookup],
    ) -> Self {
        let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
        let groups = PrimitiveKind::ALL
            .into_iter()
            .map(|kind| PrimitiveGroup::from_events(kind, events, &lookups))
            .collect();
        Self {
            total_events,
            lost_events,
            groups,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveGroup {
    pub(super) kind: PrimitiveKind,
    pub(super) events: u64,
    pub(super) objects: u64,
    pub(super) contentions: u64,
    pub(super) operations: Vec<PrimitiveOperation>,
}

impl PrimitiveGroup {
    fn from_events(
        kind: PrimitiveKind,
        events: &[seismograph::recorder::event::Event],
        lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    ) -> Self {
        let object_ids = events
            .iter()
            .filter(|event| kind.identifies(event.kind))
            .filter_map(|event| event.object_id().map(seismograph::recorder::event::ObjectId::get))
            .collect::<HashSet<_>>();
        let operations = kind
            .operations()
            .iter()
            .copied()
            .map(|operation| PrimitiveOperation::from_events(operation, events, lookups, &object_ids))
            .collect::<Vec<_>>();
        Self {
            kind,
            events: operations.iter().map(|operation| operation.events).sum(),
            objects: u64::try_from(object_ids.len()).unwrap_or(u64::MAX),
            contentions: operations
                .iter()
                .filter(|operation| operation.kind.is_contention())
                .map(|operation| operation.events)
                .sum(),
            operations,
        }
    }

    pub(super) fn sorted_operations(&self, sort: PrimitiveSort, descending: bool) -> Vec<&PrimitiveOperation> {
        let mut operations = self.operations.iter().collect::<Vec<_>>();
        operations.sort_unstable_by(|left, right| {
            let ordering = match sort {
                PrimitiveSort::Events => left.events.cmp(&right.events),
                PrimitiveSort::Objects => left.objects.cmp(&right.objects),
                PrimitiveSort::Threads => left.threads.cmp(&right.threads),
                PrimitiveSort::Hotspots => left.hotspots.len().cmp(&right.hotspots.len()),
            }
            .then_with(|| left.events.cmp(&right.events));
            if descending { ordering.reverse() } else { ordering }
        });
        operations
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveOperation {
    pub(super) kind: PrimitiveOperationKind,
    pub(super) events: u64,
    pub(super) objects: u64,
    pub(super) threads: u64,
    pub(super) hotspots: Vec<PrimitiveHotspot>,
}

impl PrimitiveOperation {
    fn from_events(
        kind: PrimitiveOperationKind,
        events: &[seismograph::recorder::event::Event],
        lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
        object_ids: &HashSet<u64>,
    ) -> Self {
        let matching = events
            .iter()
            .filter(|event| event.kind == kind.event_kind())
            .filter(|event| !kind.is_lock_poison() || event.object_id().is_some_and(|object_id| object_ids.contains(&object_id.get())))
            .collect::<Vec<_>>();
        let objects = matching
            .iter()
            .filter_map(|event| event.object_id().map(seismograph::recorder::event::ObjectId::get))
            .collect::<std::collections::HashSet<_>>();
        let threads = matching
            .iter()
            .map(|event| event.thread_id.get())
            .collect::<std::collections::HashSet<_>>();
        let mut totals = HashMap::<Vec<u64>, u64>::new();
        for event in &matching {
            let stack = event.call_stack.iter().map(|address| address.get()).collect::<Vec<_>>();
            *totals.entry(stack).or_default() += 1;
        }
        let mut hotspots = totals
            .into_iter()
            .map(|(stack, count)| PrimitiveHotspot {
                count,
                application_stack: primitive_stack(&stack, lookups, AllocationStackFilter::Application),
                complete_stack: primitive_stack(&stack, lookups, AllocationStackFilter::All),
            })
            .collect::<Vec<_>>();
        hotspots.sort_unstable_by_key(|hotspot| std::cmp::Reverse(hotspot.count));
        Self {
            kind,
            events: u64::try_from(matching.len()).unwrap_or(u64::MAX),
            objects: u64::try_from(objects.len()).unwrap_or(u64::MAX),
            threads: u64::try_from(threads.len()).unwrap_or(u64::MAX),
            hotspots,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveHotspot {
    pub(super) count: u64,
    application_stack: Vec<String>,
    complete_stack: Vec<String>,
}

impl PrimitiveHotspot {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.application_stack,
            AllocationStackFilter::All => &self.complete_stack,
        }
    }

    pub(super) fn location(&self, filter: AllocationStackFilter) -> &str {
        self.stack(filter).first().map_or("Backtraces disabled", String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveKind {
    Arc,
    Mutex,
    RwLock,
    Barrier,
    Condvar,
    Once,
    Channel,
}

impl PrimitiveKind {
    const ALL: [Self; 7] = [
        Self::Arc,
        Self::Mutex,
        Self::RwLock,
        Self::Barrier,
        Self::Condvar,
        Self::Once,
        Self::Channel,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Arc => "Arc",
            Self::Mutex => "Mutex",
            Self::RwLock => "RwLock",
            Self::Barrier => "Barrier",
            Self::Condvar => "Condvar",
            Self::Once => "OnceLock / LazyLock",
            Self::Channel => "Channel",
        }
    }

    const fn operations(self) -> &'static [PrimitiveOperationKind] {
        match self {
            Self::Arc => &PrimitiveOperationKind::ARC,
            Self::Mutex => &PrimitiveOperationKind::MUTEX,
            Self::RwLock => &PrimitiveOperationKind::RW_LOCK,
            Self::Barrier => &PrimitiveOperationKind::BARRIER,
            Self::Condvar => &PrimitiveOperationKind::CONDVAR,
            Self::Once => &PrimitiveOperationKind::ONCE,
            Self::Channel => &PrimitiveOperationKind::CHANNEL,
        }
    }

    fn identifies(self, kind: seismograph::recorder::event::EventKind) -> bool {
        self.operations()
            .iter()
            .filter(|operation| !operation.is_lock_poison())
            .any(|operation| operation.event_kind().wire_value() == kind.wire_value())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveOperationKind {
    ArcCreate,
    ArcClone,
    ArcDeref,
    ArcDrop,
    ArcRelocate,
    MutexAccess,
    MutexContention,
    MutexRelease,
    RwLockReadAccess,
    RwLockReadContention,
    RwLockReadRelease,
    RwLockWriteAccess,
    RwLockWriteContention,
    RwLockWriteRelease,
    BarrierAccess,
    BarrierContention,
    BarrierRelease,
    CondvarAccess,
    CondvarContention,
    CondvarNotify,
    OnceAccess,
    OnceContention,
    OnceInitialize,
    ChannelSend,
    ChannelSendContention,
    ChannelReceive,
    ChannelReceiveContention,
    ChannelClose,
    ChannelHighWatermark,
    LockPoisoned,
    LockPoisonObserved,
    LockPoisonCleared,
}

impl PrimitiveOperationKind {
    const ARC: [Self; 5] = [Self::ArcCreate, Self::ArcClone, Self::ArcDeref, Self::ArcDrop, Self::ArcRelocate];
    const MUTEX: [Self; 6] = [
        Self::MutexAccess,
        Self::MutexContention,
        Self::MutexRelease,
        Self::LockPoisoned,
        Self::LockPoisonObserved,
        Self::LockPoisonCleared,
    ];
    const RW_LOCK: [Self; 9] = [
        Self::RwLockReadAccess,
        Self::RwLockReadContention,
        Self::RwLockReadRelease,
        Self::RwLockWriteAccess,
        Self::RwLockWriteContention,
        Self::RwLockWriteRelease,
        Self::LockPoisoned,
        Self::LockPoisonObserved,
        Self::LockPoisonCleared,
    ];
    const BARRIER: [Self; 3] = [Self::BarrierAccess, Self::BarrierContention, Self::BarrierRelease];
    const CONDVAR: [Self; 3] = [Self::CondvarAccess, Self::CondvarContention, Self::CondvarNotify];
    const ONCE: [Self; 3] = [Self::OnceAccess, Self::OnceContention, Self::OnceInitialize];
    const CHANNEL: [Self; 6] = [
        Self::ChannelSend,
        Self::ChannelSendContention,
        Self::ChannelReceive,
        Self::ChannelReceiveContention,
        Self::ChannelClose,
        Self::ChannelHighWatermark,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ArcCreate => "Create",
            Self::ArcClone => "Clone",
            Self::ArcDeref => "Deref",
            Self::ArcDrop => "Final drop",
            Self::ArcRelocate => "Relocate",
            Self::MutexAccess => "Acquisition",
            Self::MutexContention | Self::OnceContention => "Contention",
            Self::MutexRelease => "Release",
            Self::RwLockReadAccess => "Read acquisition",
            Self::RwLockReadContention => "Read contention",
            Self::RwLockReadRelease => "Read release",
            Self::RwLockWriteAccess => "Write acquisition",
            Self::RwLockWriteContention => "Write contention",
            Self::RwLockWriteRelease => "Write release",
            Self::BarrierAccess | Self::CondvarAccess => "Completed wait",
            Self::BarrierContention | Self::CondvarContention => "Blocked wait",
            Self::BarrierRelease => "Generation release",
            Self::CondvarNotify => "Notification",
            Self::OnceAccess => "Access",
            Self::OnceInitialize => "Initialization",
            Self::ChannelSend => "Send",
            Self::ChannelReceive => "Receive",
            Self::ChannelSendContention => "Send contention",
            Self::ChannelReceiveContention => "Receive contention",
            Self::ChannelClose => "Close",
            Self::ChannelHighWatermark => "High watermark",
            Self::LockPoisoned => "Poisoned",
            Self::LockPoisonObserved => "Poison observed",
            Self::LockPoisonCleared => "Poison cleared",
        }
    }

    const fn event_kind(self) -> seismograph::recorder::event::EventKind {
        use seismograph::recorder::event::EventKind;
        match self {
            Self::ArcCreate => EventKind::ArcCreate,
            Self::ArcClone => EventKind::ArcClone,
            Self::ArcDeref => EventKind::ArcDeref,
            Self::ArcDrop => EventKind::ArcDrop,
            Self::ArcRelocate => EventKind::ArcRelocate,
            Self::MutexAccess => EventKind::MutexAccess,
            Self::MutexContention => EventKind::MutexContention,
            Self::MutexRelease => EventKind::MutexRelease,
            Self::RwLockReadAccess => EventKind::RwLockReadAccess,
            Self::RwLockReadContention => EventKind::RwLockReadContention,
            Self::RwLockReadRelease => EventKind::RwLockReadRelease,
            Self::RwLockWriteAccess => EventKind::RwLockWriteAccess,
            Self::RwLockWriteContention => EventKind::RwLockWriteContention,
            Self::RwLockWriteRelease => EventKind::RwLockWriteRelease,
            Self::BarrierAccess => EventKind::BarrierAccess,
            Self::BarrierContention => EventKind::BarrierContention,
            Self::BarrierRelease => EventKind::BarrierRelease,
            Self::CondvarAccess => EventKind::CondvarAccess,
            Self::CondvarContention => EventKind::CondvarContention,
            Self::CondvarNotify => EventKind::CondvarNotify,
            Self::OnceAccess => EventKind::OnceAccess,
            Self::OnceContention => EventKind::OnceContention,
            Self::OnceInitialize => EventKind::OnceInitialize,
            Self::ChannelSend => EventKind::ChannelSend,
            Self::ChannelSendContention => EventKind::ChannelSendContention,
            Self::ChannelReceive => EventKind::ChannelReceive,
            Self::ChannelReceiveContention => EventKind::ChannelReceiveContention,
            Self::ChannelClose => EventKind::ChannelClose,
            Self::ChannelHighWatermark => EventKind::ChannelHighWatermark,
            Self::LockPoisoned => EventKind::LockPoisoned,
            Self::LockPoisonObserved => EventKind::LockPoisonObserved,
            Self::LockPoisonCleared => EventKind::LockPoisonCleared,
        }
    }

    const fn is_lock_poison(self) -> bool {
        matches!(self, Self::LockPoisoned | Self::LockPoisonObserved | Self::LockPoisonCleared)
    }

    pub(super) const fn is_contention(self) -> bool {
        matches!(
            self,
            Self::MutexContention
                | Self::RwLockReadContention
                | Self::RwLockWriteContention
                | Self::BarrierContention
                | Self::CondvarContention
                | Self::OnceContention
                | Self::ChannelSendContention
                | Self::ChannelReceiveContention
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveSort {
    Events,
    Objects,
    Threads,
    Hotspots,
}

impl PrimitiveSort {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Events => Self::Objects,
            Self::Objects => Self::Threads,
            Self::Threads => Self::Hotspots,
            Self::Hotspots => Self::Events,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Events => Self::Hotspots,
            Self::Objects => Self::Events,
            Self::Threads => Self::Objects,
            Self::Hotspots => Self::Threads,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadSnapshot {
    pub(super) threads: Vec<ThreadSummary>,
}

impl ThreadSnapshot {
    fn from_events(decoded: &seismograph::recorder::event::Events, addresses: &[seismograph_rallocator::callers::AddressLookup]) -> Self {
        #[derive(Default)]
        struct Metadata {
            name: String,
            total_events: u64,
            lost_events: u64,
        }

        let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
        let mut metadata = decoded
            .threads
            .iter()
            .map(|thread| {
                (
                    thread.thread_id.get(),
                    Metadata {
                        name: thread.name.clone(),
                        total_events: thread.total_events,
                        lost_events: thread.lost_events,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut events_by_thread = HashMap::<u64, Vec<&seismograph::recorder::event::Event>>::new();
        let mut events_by_object = HashMap::<u64, Vec<&seismograph::recorder::event::Event>>::new();
        for event in &decoded.events {
            metadata.entry(event.thread_id.get()).or_default();
            events_by_thread.entry(event.thread_id.get()).or_default().push(event);
            if let Some(object_id) = event.object_id() {
                events_by_object.entry(object_id.get()).or_default().push(event);
            }
        }
        let threads = metadata
            .into_iter()
            .map(|(thread_id, metadata)| {
                let events = events_by_thread.get(&thread_id).map_or(&[][..], Vec::as_slice);
                let retained_events = u64::try_from(events.len()).unwrap_or(u64::MAX);
                let operations = ThreadOperationKind::ALL
                    .into_iter()
                    .map(|kind| ThreadOperation::from_events(kind, thread_id, events, &events_by_object, &decoded.threads, &lookups))
                    .collect();
                ThreadSummary {
                    thread_id,
                    name: metadata.name,
                    total_events: metadata.total_events.max(retained_events),
                    retained_events,
                    lost_events: metadata.lost_events,
                    operations,
                }
            })
            .collect();
        Self { threads }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadSummary {
    pub(super) thread_id: u64,
    pub(super) name: String,
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) operations: Vec<ThreadOperation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadOperation {
    pub(super) kind: ThreadOperationKind,
    pub(super) events: u64,
    pub(super) objects: u64,
    pub(super) participants: Vec<ThreadParticipant>,
}

impl ThreadOperation {
    fn from_events(
        kind: ThreadOperationKind,
        thread_id: u64,
        events: &[&seismograph::recorder::event::Event],
        events_by_object: &HashMap<u64, Vec<&seismograph::recorder::event::Event>>,
        thread_logs: &[seismograph::recorder::thread::ThreadLog],
        lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    ) -> Self {
        #[derive(Default)]
        struct ParticipantTotal<'a> {
            objects: BTreeMap<u64, Vec<&'a seismograph::recorder::event::Event>>,
            events: u64,
        }

        let matching = events
            .iter()
            .copied()
            .filter(|event| event.kind == kind.event_kind())
            .collect::<Vec<_>>();
        let objects = matching
            .iter()
            .filter_map(|event| event.object_id().map(seismograph::recorder::event::ObjectId::get))
            .collect::<HashSet<_>>();
        let mut totals = BTreeMap::<u64, ParticipantTotal<'_>>::new();
        for object_id in &objects {
            for event in events_by_object.get(object_id).into_iter().flatten() {
                let participant_id = event.thread_id.get();
                if participant_id == thread_id || !kind.is_related(event.kind) {
                    continue;
                }
                let total = totals.entry(participant_id).or_default();
                total.objects.entry(*object_id).or_default().push(*event);
                total.events = total.events.saturating_add(1);
            }
        }
        let participants = totals
            .into_iter()
            .map(|(participant_id, total)| {
                let mut participant_objects = total
                    .objects
                    .into_iter()
                    .map(|(object_id, related_events)| {
                        let selected_events = matching
                            .iter()
                            .copied()
                            .filter(|event| event.object_id().is_some_and(|id| id.get() == object_id))
                            .collect::<Vec<_>>();
                        ThreadObject::from_events(kind, object_id, &selected_events, &related_events, lookups)
                    })
                    .collect::<Vec<_>>();
                participant_objects.sort_unstable_by(|left, right| {
                    right
                        .hotness()
                        .cmp(&left.hotness())
                        .then_with(|| right.related_events.cmp(&left.related_events))
                        .then_with(|| left.object_id.cmp(&right.object_id))
                });
                ThreadParticipant {
                    thread_id: participant_id,
                    name: thread_logs
                        .iter()
                        .find(|thread| thread.thread_id.get() == participant_id)
                        .map_or_else(String::new, |thread| thread.name.clone()),
                    events: total.events,
                    objects: participant_objects,
                }
            })
            .collect();
        Self {
            kind,
            events: u64::try_from(matching.len()).unwrap_or(u64::MAX),
            objects: u64::try_from(objects.len()).unwrap_or(u64::MAX),
            participants,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadParticipant {
    pub(super) thread_id: u64,
    pub(super) name: String,
    pub(super) events: u64,
    pub(super) objects: Vec<ThreadObject>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadObject {
    pub(super) object_id: u64,
    pub(super) selected_events: u64,
    pub(super) related_events: u64,
    selected_stacks: Vec<ThreadStack>,
    related_stacks: Vec<ThreadStack>,
}

impl ThreadObject {
    fn from_events(
        kind: ThreadOperationKind,
        object_id: u64,
        selected_events: &[&seismograph::recorder::event::Event],
        related_events: &[&seismograph::recorder::event::Event],
        lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    ) -> Self {
        Self {
            object_id,
            selected_events: u64::try_from(selected_events.len()).unwrap_or(u64::MAX),
            related_events: u64::try_from(related_events.len()).unwrap_or(u64::MAX),
            selected_stacks: thread_stacks(selected_events.iter().copied(), kind, lookups),
            related_stacks: thread_stacks(related_events.iter().copied(), kind, lookups),
        }
    }

    pub(super) const fn hotness(&self) -> u64 {
        self.selected_events.saturating_add(self.related_events)
    }

    pub(super) fn selected_stack(&self) -> Option<&ThreadStack> {
        self.selected_stacks.first()
    }

    pub(super) fn related_stack(&self) -> Option<&ThreadStack> {
        self.related_stacks.first()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadStack {
    pub(super) count: u64,
    application_stack: Vec<String>,
    complete_stack: Vec<String>,
}

impl ThreadStack {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.application_stack,
            AllocationStackFilter::All => &self.complete_stack,
        }
    }
}

fn thread_stacks<'a>(
    events: impl Iterator<Item = &'a seismograph::recorder::event::Event>,
    kind: ThreadOperationKind,
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
) -> Vec<ThreadStack> {
    let mut totals = HashMap::<Vec<u64>, u64>::new();
    for event in events {
        let stack = event.call_stack.iter().map(|address| address.get()).collect::<Vec<_>>();
        *totals.entry(stack).or_default() += 1;
    }
    let stack = |addresses: &[u64], filter| {
        if kind.is_allocation() {
            hotspot_stack(addresses, lookups, filter)
        } else {
            primitive_stack(addresses, lookups, filter)
        }
    };
    let mut stacks = totals
        .into_iter()
        .map(|(addresses, count)| ThreadStack {
            count,
            application_stack: stack(&addresses, AllocationStackFilter::Application),
            complete_stack: stack(&addresses, AllocationStackFilter::All),
        })
        .collect::<Vec<_>>();
    stacks.sort_unstable_by_key(|stack| std::cmp::Reverse(stack.count));
    stacks
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ThreadOperationKind {
    Allocation,
    Deallocation,
    ArcCreate,
    ArcClone,
    ArcDeref,
    ArcDrop,
    ArcRelocate,
    MutexAccess,
    MutexContention,
    MutexRelease,
    RwLockReadAccess,
    RwLockReadContention,
    RwLockReadRelease,
    RwLockWriteAccess,
    RwLockWriteContention,
    RwLockWriteRelease,
    BarrierAccess,
    BarrierContention,
    BarrierRelease,
    CondvarAccess,
    CondvarContention,
    CondvarNotify,
    OnceAccess,
    OnceContention,
    OnceInitialize,
    ChannelSend,
    ChannelSendContention,
    ChannelReceive,
    ChannelReceiveContention,
    ChannelClose,
    ChannelHighWatermark,
    LockPoisoned,
    LockPoisonObserved,
    LockPoisonCleared,
}

impl ThreadOperationKind {
    const ALL: [Self; 34] = [
        Self::Allocation,
        Self::Deallocation,
        Self::ArcCreate,
        Self::ArcClone,
        Self::ArcDeref,
        Self::ArcDrop,
        Self::ArcRelocate,
        Self::MutexAccess,
        Self::MutexContention,
        Self::MutexRelease,
        Self::RwLockReadAccess,
        Self::RwLockReadContention,
        Self::RwLockReadRelease,
        Self::RwLockWriteAccess,
        Self::RwLockWriteContention,
        Self::RwLockWriteRelease,
        Self::BarrierAccess,
        Self::BarrierContention,
        Self::BarrierRelease,
        Self::CondvarAccess,
        Self::CondvarContention,
        Self::CondvarNotify,
        Self::OnceAccess,
        Self::OnceContention,
        Self::OnceInitialize,
        Self::ChannelSend,
        Self::ChannelSendContention,
        Self::ChannelReceive,
        Self::ChannelReceiveContention,
        Self::ChannelClose,
        Self::ChannelHighWatermark,
        Self::LockPoisoned,
        Self::LockPoisonObserved,
        Self::LockPoisonCleared,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Allocation => "Allocation",
            Self::Deallocation => "Deallocation",
            Self::ArcCreate => "Arc create",
            Self::ArcClone => "Arc clone",
            Self::ArcDeref => "Arc deref",
            Self::ArcDrop => "Arc final drop",
            Self::ArcRelocate => "Arc relocation",
            Self::MutexAccess => "Mutex acquisition",
            Self::MutexContention => "Mutex contention",
            Self::MutexRelease => "Mutex release",
            Self::RwLockReadAccess => "RwLock read acquisition",
            Self::RwLockReadContention => "RwLock read contention",
            Self::RwLockReadRelease => "RwLock read release",
            Self::RwLockWriteAccess => "RwLock write acquisition",
            Self::RwLockWriteContention => "RwLock write contention",
            Self::RwLockWriteRelease => "RwLock write release",
            Self::BarrierAccess => "Barrier completed wait",
            Self::BarrierContention => "Barrier blocked wait",
            Self::BarrierRelease => "Barrier generation release",
            Self::CondvarAccess => "Condvar completed wait",
            Self::CondvarContention => "Condvar blocked wait",
            Self::CondvarNotify => "Condvar notification",
            Self::OnceAccess => "Once value access",
            Self::OnceContention => "Once initialization contention",
            Self::OnceInitialize => "Once initialization",
            Self::ChannelSend => "Channel send",
            Self::ChannelSendContention => "Channel send contention",
            Self::ChannelReceive => "Channel receive",
            Self::ChannelReceiveContention => "Channel receive contention",
            Self::ChannelClose => "Channel close",
            Self::ChannelHighWatermark => "Channel high watermark",
            Self::LockPoisoned => "Lock poisoned",
            Self::LockPoisonObserved => "Lock poison observed",
            Self::LockPoisonCleared => "Lock poison cleared",
        }
    }

    pub(super) const fn relationship_label(self) -> &'static str {
        match self {
            Self::Allocation => "Threads that deallocated these allocations",
            Self::Deallocation => "Threads that created these allocations",
            Self::ArcCreate | Self::ArcClone | Self::ArcDeref | Self::ArcDrop | Self::ArcRelocate => {
                "Other threads observed on the same Arc objects"
            }
            Self::MutexAccess | Self::MutexContention | Self::MutexRelease => "Other threads observed on the same Mutex objects",
            Self::RwLockReadAccess
            | Self::RwLockReadContention
            | Self::RwLockReadRelease
            | Self::RwLockWriteAccess
            | Self::RwLockWriteContention
            | Self::RwLockWriteRelease => "Other threads observed on the same RwLock objects",
            Self::BarrierAccess | Self::BarrierContention | Self::BarrierRelease => "Other threads observed on the same Barrier objects",
            Self::CondvarAccess | Self::CondvarContention | Self::CondvarNotify => "Other threads observed on the same Condvar objects",
            Self::OnceAccess | Self::OnceContention | Self::OnceInitialize => "Other threads observed on the same once-initialized objects",
            Self::ChannelSend
            | Self::ChannelSendContention
            | Self::ChannelReceive
            | Self::ChannelReceiveContention
            | Self::ChannelClose
            | Self::ChannelHighWatermark => "Other threads observed on the same Channel objects",
            Self::LockPoisoned | Self::LockPoisonObserved | Self::LockPoisonCleared => {
                "Other threads observed on the same Mutex or RwLock objects"
            }
        }
    }

    pub(super) const fn is_contention(self) -> bool {
        matches!(
            self,
            Self::MutexContention
                | Self::RwLockReadContention
                | Self::RwLockWriteContention
                | Self::BarrierContention
                | Self::CondvarContention
                | Self::OnceContention
                | Self::ChannelSendContention
                | Self::ChannelReceiveContention
        )
    }

    const fn is_allocation(self) -> bool {
        matches!(self, Self::Allocation | Self::Deallocation)
    }

    const fn event_kind(self) -> seismograph::recorder::event::EventKind {
        use seismograph::recorder::event::EventKind;
        match self {
            Self::Allocation => EventKind::Allocation,
            Self::Deallocation => EventKind::Deallocation,
            Self::ArcCreate => EventKind::ArcCreate,
            Self::ArcClone => EventKind::ArcClone,
            Self::ArcDeref => EventKind::ArcDeref,
            Self::ArcDrop => EventKind::ArcDrop,
            Self::ArcRelocate => EventKind::ArcRelocate,
            Self::MutexAccess => EventKind::MutexAccess,
            Self::MutexContention => EventKind::MutexContention,
            Self::MutexRelease => EventKind::MutexRelease,
            Self::RwLockReadAccess => EventKind::RwLockReadAccess,
            Self::RwLockReadContention => EventKind::RwLockReadContention,
            Self::RwLockReadRelease => EventKind::RwLockReadRelease,
            Self::RwLockWriteAccess => EventKind::RwLockWriteAccess,
            Self::RwLockWriteContention => EventKind::RwLockWriteContention,
            Self::RwLockWriteRelease => EventKind::RwLockWriteRelease,
            Self::BarrierAccess => EventKind::BarrierAccess,
            Self::BarrierContention => EventKind::BarrierContention,
            Self::BarrierRelease => EventKind::BarrierRelease,
            Self::CondvarAccess => EventKind::CondvarAccess,
            Self::CondvarContention => EventKind::CondvarContention,
            Self::CondvarNotify => EventKind::CondvarNotify,
            Self::OnceAccess => EventKind::OnceAccess,
            Self::OnceContention => EventKind::OnceContention,
            Self::OnceInitialize => EventKind::OnceInitialize,
            Self::ChannelSend => EventKind::ChannelSend,
            Self::ChannelSendContention => EventKind::ChannelSendContention,
            Self::ChannelReceive => EventKind::ChannelReceive,
            Self::ChannelReceiveContention => EventKind::ChannelReceiveContention,
            Self::ChannelClose => EventKind::ChannelClose,
            Self::ChannelHighWatermark => EventKind::ChannelHighWatermark,
            Self::LockPoisoned => EventKind::LockPoisoned,
            Self::LockPoisonObserved => EventKind::LockPoisonObserved,
            Self::LockPoisonCleared => EventKind::LockPoisonCleared,
        }
    }

    const fn is_related(self, kind: seismograph::recorder::event::EventKind) -> bool {
        use seismograph::recorder::event::EventKind;
        match self {
            Self::Allocation => matches!(kind, EventKind::Deallocation),
            Self::Deallocation => matches!(kind, EventKind::Allocation),
            Self::ArcCreate | Self::ArcClone | Self::ArcDeref | Self::ArcDrop | Self::ArcRelocate => matches!(
                kind,
                EventKind::ArcCreate | EventKind::ArcClone | EventKind::ArcDeref | EventKind::ArcDrop | EventKind::ArcRelocate
            ),
            Self::MutexAccess | Self::MutexContention | Self::MutexRelease => {
                matches!(
                    kind,
                    EventKind::MutexAccess
                        | EventKind::MutexContention
                        | EventKind::MutexRelease
                        | EventKind::LockPoisoned
                        | EventKind::LockPoisonObserved
                        | EventKind::LockPoisonCleared
                )
            }
            Self::RwLockReadAccess
            | Self::RwLockReadContention
            | Self::RwLockReadRelease
            | Self::RwLockWriteAccess
            | Self::RwLockWriteContention
            | Self::RwLockWriteRelease => matches!(
                kind,
                EventKind::RwLockReadAccess
                    | EventKind::RwLockReadContention
                    | EventKind::RwLockReadRelease
                    | EventKind::RwLockWriteAccess
                    | EventKind::RwLockWriteContention
                    | EventKind::RwLockWriteRelease
                    | EventKind::LockPoisoned
                    | EventKind::LockPoisonObserved
                    | EventKind::LockPoisonCleared
            ),
            Self::BarrierAccess | Self::BarrierContention | Self::BarrierRelease => matches!(
                kind,
                EventKind::BarrierAccess | EventKind::BarrierContention | EventKind::BarrierRelease
            ),
            Self::CondvarAccess | Self::CondvarContention | Self::CondvarNotify => matches!(
                kind,
                EventKind::CondvarAccess | EventKind::CondvarContention | EventKind::CondvarNotify
            ),
            Self::OnceAccess | Self::OnceContention | Self::OnceInitialize => {
                matches!(kind, EventKind::OnceAccess | EventKind::OnceContention | EventKind::OnceInitialize)
            }
            Self::ChannelSend
            | Self::ChannelSendContention
            | Self::ChannelReceive
            | Self::ChannelReceiveContention
            | Self::ChannelClose
            | Self::ChannelHighWatermark => matches!(
                kind,
                EventKind::ChannelSend
                    | EventKind::ChannelSendContention
                    | EventKind::ChannelReceive
                    | EventKind::ChannelReceiveContention
                    | EventKind::ChannelClose
                    | EventKind::ChannelHighWatermark
            ),
            Self::LockPoisoned | Self::LockPoisonObserved | Self::LockPoisonCleared => matches!(
                kind,
                EventKind::MutexAccess
                    | EventKind::MutexContention
                    | EventKind::MutexRelease
                    | EventKind::RwLockReadAccess
                    | EventKind::RwLockReadContention
                    | EventKind::RwLockReadRelease
                    | EventKind::RwLockWriteAccess
                    | EventKind::RwLockWriteContention
                    | EventKind::RwLockWriteRelease
                    | EventKind::LockPoisoned
                    | EventKind::LockPoisonObserved
                    | EventKind::LockPoisonCleared
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AllocationSort {
    Allocations,
    AllocatedBytes,
    AverageBytes,
    LiveAllocations,
    LiveBytes,
}

impl AllocationSort {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Allocations => Self::AllocatedBytes,
            Self::AllocatedBytes => Self::AverageBytes,
            Self::AverageBytes => Self::LiveAllocations,
            Self::LiveAllocations => Self::LiveBytes,
            Self::LiveBytes => Self::Allocations,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Allocations => Self::LiveBytes,
            Self::AllocatedBytes => Self::Allocations,
            Self::AverageBytes => Self::AllocatedBytes,
            Self::LiveAllocations => Self::AverageBytes,
            Self::LiveBytes => Self::LiveAllocations,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemorySnapshot {
    pub(super) live_bytes: u64,
    pub(super) peak_live_bytes: u64,
    pub(super) mapped_bytes: u64,
    pub(super) allocations: u64,
    pub(super) reserved_bytes: u64,
    pub(super) used_slices: u64,
    pub(super) free_slices: u64,
    pub(super) slice_bytes: u64,
    pub(super) small_slices: u64,
    pub(super) medium_slices: u64,
    pub(super) bump_slices: u64,
    pub(super) unknown_slices: u64,
    pub(super) regions: Vec<MemoryRegion>,
    pub(super) size_classes: Vec<MemorySizeClass>,
    pub(super) medium_allocations: MediumAllocations,
    pub(super) tiers: Vec<MemoryTierData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemoryRegion {
    pub(super) index: u32,
    pub(super) reserved_bytes: u64,
    pub(super) used_slices: u64,
    pub(super) free_slices: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemorySizeClass {
    pub(super) block_bytes: u64,
    pub(super) live_allocations: u64,
    pub(super) capacity_blocks: u64,
    pub(super) requested_bytes: u64,
    pub(super) usable_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct MediumAllocations {
    pub(super) count: u64,
    pub(super) requested_bytes: u64,
    pub(super) usable_bytes: u64,
    pub(super) span_slices: u64,
    pub(super) largest_requested_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum MemoryTier {
    Small,
    Medium,
    Direct,
}

impl MemoryTier {
    const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Direct];

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Small => 0,
            Self::Medium => 1,
            Self::Direct => 2,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Direct => "Direct",
        }
    }

    pub(super) const fn next(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Direct,
            Self::Direct => Self::Small,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Small => Self::Direct,
            Self::Medium => Self::Small,
            Self::Direct => Self::Medium,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemoryTierData {
    pub(super) kind: MemoryTier,
    pub(super) current_allocations: u64,
    pub(super) current_bytes: u64,
    pub(super) buckets: Vec<MemoryBucket>,
}

impl MemoryTierData {
    pub(super) fn retained_allocations(&self) -> u64 {
        self.buckets.iter().map(|bucket| bucket.allocations).sum()
    }

    pub(super) fn retained_bytes(&self) -> u64 {
        self.buckets.iter().map(|bucket| bucket.allocated_bytes).sum()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemoryBucket {
    pub(super) lower_bytes: u64,
    pub(super) upper_bytes: u64,
    pub(super) allocations: u64,
    pub(super) allocated_bytes: u64,
    pub(super) live_allocations: u64,
    pub(super) live_bytes: u64,
    pub(super) topology_live_allocations: Option<u64>,
    pub(super) capacity_blocks: Option<u64>,
    pub(super) requested_bytes: Option<u64>,
    pub(super) usable_bytes: Option<u64>,
    pub(super) hotspots: Vec<AllocationHotspot>,
}

#[derive(Default)]
struct MemoryHotspotTotal {
    allocations: u64,
    allocated_bytes: u64,
    live_allocations: u64,
    live_bytes: u64,
}

#[derive(Default)]
struct MemoryBucketTotal {
    allocations: u64,
    allocated_bytes: u64,
    live_allocations: u64,
    live_bytes: u64,
    hotspots: HashMap<Vec<u64>, MemoryHotspotTotal>,
}

impl MemorySnapshot {
    pub(super) fn from_snapshot(snapshot: &seismograph_rallocator::snapshot::Snapshot) -> Self {
        let mut small_slices = 0;
        let mut medium_slices = 0;
        let mut bump_slices = 0;
        let mut unknown_slices = 0;
        let mut medium_allocations = MediumAllocations::default();
        for slice in snapshot.topology.iter().flat_map(|region| &region.slices) {
            match slice.kind {
                seismograph_rallocator::topology::SliceKind::Small => small_slices += 1,
                seismograph_rallocator::topology::SliceKind::Medium => {
                    medium_slices += 1;
                    medium_allocations.count += 1;
                    medium_allocations.requested_bytes = medium_allocations.requested_bytes.saturating_add(slice.requested_bytes);
                    medium_allocations.usable_bytes = medium_allocations.usable_bytes.saturating_add(slice.usable_bytes);
                    medium_allocations.span_slices = medium_allocations.span_slices.saturating_add(u64::from(slice.span_slices));
                    medium_allocations.largest_requested_bytes = medium_allocations.largest_requested_bytes.max(slice.requested_bytes);
                }
                seismograph_rallocator::topology::SliceKind::MediumContinuation => medium_slices += 1,
                seismograph_rallocator::topology::SliceKind::Bump => bump_slices += 1,
                seismograph_rallocator::topology::SliceKind::Unknown => unknown_slices += 1,
            }
        }
        let mut size_classes = snapshot
            .size_classes
            .iter()
            .map(|class| {
                let capacity_blocks = snapshot
                    .topology
                    .iter()
                    .flat_map(|region| &region.slices)
                    .flat_map(|slice| &slice.segments)
                    .filter(|segment| u64::from(segment.class_index) == u64::from(class.class_index))
                    .map(|segment| u64::from(segment.usable_blocks))
                    .sum();
                MemorySizeClass {
                    block_bytes: class.block_bytes,
                    live_allocations: class.live_allocations.value,
                    capacity_blocks,
                    requested_bytes: class.requested_bytes.value,
                    usable_bytes: class.usable_bytes.value,
                }
            })
            .collect::<Vec<_>>();
        size_classes.sort_unstable_by_key(|class| class.block_bytes);
        let tiers = memory_tiers(snapshot, &size_classes, &medium_allocations);
        Self {
            live_bytes: snapshot.stats.live_bytes,
            peak_live_bytes: snapshot.stats.peak_live_bytes,
            mapped_bytes: snapshot.stats.mapped_bytes,
            allocations: snapshot.stats.allocations,
            reserved_bytes: snapshot.regions.iter().map(|region| region.reserved_bytes).sum(),
            used_slices: snapshot.regions.iter().map(|region| region.used_slices).sum(),
            free_slices: snapshot.regions.iter().map(|region| region.free_slices).sum(),
            slice_bytes: snapshot.topology.first().map_or(0, |region| region.slice_bytes),
            small_slices,
            medium_slices,
            bump_slices,
            unknown_slices,
            regions: snapshot
                .regions
                .iter()
                .map(|region| MemoryRegion {
                    index: region.region_index,
                    reserved_bytes: region.reserved_bytes,
                    used_slices: region.used_slices,
                    free_slices: region.free_slices,
                })
                .collect(),
            size_classes,
            medium_allocations,
            tiers,
        }
    }
}

fn memory_tiers(
    snapshot: &seismograph_rallocator::snapshot::Snapshot,
    size_classes: &[MemorySizeClass],
    medium_allocations: &MediumAllocations,
) -> Vec<MemoryTierData> {
    let lookups = snapshot
        .addresses
        .iter()
        .map(|lookup| (lookup.address, lookup))
        .collect::<HashMap<_, _>>();
    let mut totals = retained_memory_totals(snapshot, size_classes);
    let small_current_allocations = size_classes.iter().map(|class| class.live_allocations).sum();
    let small_current_bytes = size_classes.iter().map(|class| class.requested_bytes).sum();
    MemoryTier::ALL
        .into_iter()
        .map(|kind| {
            let mut tier_totals = totals.remove(&kind).unwrap_or_default();
            let buckets: Vec<MemoryBucket> = match kind {
                MemoryTier::Small => {
                    let mut lower = 1;
                    size_classes
                        .iter()
                        .map(|class| {
                            let bucket = memory_bucket(
                                lower,
                                class.block_bytes,
                                tier_totals.remove(&class.block_bytes).unwrap_or_default(),
                                &lookups,
                            );
                            lower = class.block_bytes.saturating_add(1);
                            MemoryBucket {
                                topology_live_allocations: Some(class.live_allocations),
                                capacity_blocks: Some(class.capacity_blocks),
                                requested_bytes: Some(class.requested_bytes),
                                usable_bytes: Some(class.usable_bytes),
                                ..bucket
                            }
                        })
                        .collect()
                }
                MemoryTier::Medium | MemoryTier::Direct => tier_totals
                    .into_iter()
                    .map(|(bucket, total)| {
                        let (lower, upper) = histogram_bounds(bucket);
                        memory_bucket(lower, upper, total, &lookups)
                    })
                    .collect(),
            };
            let (current_allocations, current_bytes) = match kind {
                MemoryTier::Small => (small_current_allocations, small_current_bytes),
                MemoryTier::Medium => (medium_allocations.count, medium_allocations.requested_bytes),
                MemoryTier::Direct => {
                    let current_allocations = buckets.iter().map(|bucket| bucket.live_allocations).sum();
                    let current_bytes = buckets.iter().map(|bucket| bucket.live_bytes).sum();
                    (current_allocations, current_bytes)
                }
            };
            MemoryTierData {
                kind,
                current_allocations,
                current_bytes,
                buckets,
            }
        })
        .collect()
}

fn retained_memory_totals(
    snapshot: &seismograph_rallocator::snapshot::Snapshot,
    size_classes: &[MemorySizeClass],
) -> BTreeMap<MemoryTier, BTreeMap<u64, MemoryBucketTotal>> {
    use seismograph_rallocator::callers::EventKind;

    const MAX_SMALL_ALIGNMENT_BYTES: u64 = 4 * 1024;

    let Some(callers) = &snapshot.callers else {
        return BTreeMap::new();
    };
    let deallocated = callers
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Deallocated)
        .map(|event| (event.thread_log_id, event.allocation_id))
        .collect::<HashSet<_>>();
    let maximum_small = size_classes.last().map_or(0, |class| class.block_bytes);
    let medium_slice = snapshot.topology.first().map_or(64 * 1024, |region| region.slice_bytes);
    let medium_region = snapshot.topology.first().map_or(1024 * 1024 * 1024, |region| region.region_bytes);
    let mut totals = BTreeMap::<MemoryTier, BTreeMap<u64, MemoryBucketTotal>>::new();
    for event in callers.events.iter().filter(|event| event.kind == EventKind::Allocated) {
        let tier = allocation_tier(
            event.size,
            event.align,
            maximum_small,
            MAX_SMALL_ALIGNMENT_BYTES,
            medium_slice,
            medium_region,
        );
        let bucket = match tier {
            MemoryTier::Small => size_classes
                .iter()
                .find(|class| class.block_bytes >= event.size.max(event.align).max(1))
                .map_or(maximum_small, |class| class.block_bytes),
            MemoryTier::Medium | MemoryTier::Direct => u64::from(histogram_bucket(event.size)),
        };
        let total = totals.entry(tier).or_default().entry(bucket).or_default();
        total.allocations = total.allocations.saturating_add(1);
        total.allocated_bytes = total.allocated_bytes.saturating_add(event.size);
        let hotspot = total.hotspots.entry(event.call_stack.clone()).or_default();
        hotspot.allocations = hotspot.allocations.saturating_add(1);
        hotspot.allocated_bytes = hotspot.allocated_bytes.saturating_add(event.size);
        if !deallocated.contains(&(event.thread_log_id, event.allocation_id)) {
            total.live_allocations = total.live_allocations.saturating_add(1);
            total.live_bytes = total.live_bytes.saturating_add(event.size);
            hotspot.live_allocations = hotspot.live_allocations.saturating_add(1);
            hotspot.live_bytes = hotspot.live_bytes.saturating_add(event.size);
        }
    }
    totals
}

fn allocation_tier(
    size: u64,
    align: u64,
    maximum_small: u64,
    maximum_small_alignment: u64,
    medium_slice: u64,
    medium_region: u64,
) -> MemoryTier {
    let required_small = size.max(align).max(1);
    if align <= maximum_small_alignment && required_small <= maximum_small {
        MemoryTier::Small
    } else {
        let medium_slices = size.max(1).saturating_add(medium_slice.saturating_sub(1)) / medium_slice.max(1);
        if align <= medium_slice && medium_slices.saturating_mul(medium_slice) <= medium_region {
            MemoryTier::Medium
        } else {
            MemoryTier::Direct
        }
    }
}

fn histogram_bucket(size: u64) -> u32 {
    if size == 0 { 0 } else { u64::BITS - size.leading_zeros() }
}

fn histogram_bounds(bucket: u64) -> (u64, u64) {
    if bucket == 0 {
        return (0, 0);
    }
    let shift = u32::try_from(bucket.saturating_sub(1)).unwrap_or(u32::MAX).min(63);
    let lower = 1_u64 << shift;
    let upper = if shift == 63 {
        u64::MAX
    } else {
        (1_u64 << (shift + 1)).saturating_sub(1)
    };
    (lower, upper)
}

fn memory_bucket(
    lower_bytes: u64,
    upper_bytes: u64,
    total: MemoryBucketTotal,
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
) -> MemoryBucket {
    let mut hotspots = total
        .hotspots
        .into_iter()
        .map(|(stack, total)| AllocationHotspot {
            allocations: total.allocations,
            allocated_bytes: total.allocated_bytes,
            live_allocations: total.live_allocations,
            live_bytes: total.live_bytes,
            application_stack: hotspot_stack(&stack, lookups, AllocationStackFilter::Application),
            complete_stack: hotspot_stack(&stack, lookups, AllocationStackFilter::All),
        })
        .collect::<Vec<_>>();
    hotspots.sort_unstable_by(|left, right| {
        right
            .allocations
            .cmp(&left.allocations)
            .then_with(|| right.allocated_bytes.cmp(&left.allocated_bytes))
    });
    MemoryBucket {
        lower_bytes,
        upper_bytes,
        allocations: total.allocations,
        allocated_bytes: total.allocated_bytes,
        live_allocations: total.live_allocations,
        live_bytes: total.live_bytes,
        topology_live_allocations: None,
        capacity_blocks: None,
        requested_bytes: None,
        usable_bytes: None,
        hotspots,
    }
}

impl AllocationSnapshot {
    pub(super) fn from_snapshot(snapshot: &seismograph_rallocator::snapshot::Snapshot) -> Self {
        use seismograph_rallocator::callers::EventKind;

        #[derive(Default)]
        struct Total {
            allocations: u64,
            allocated_bytes: u64,
            live_allocations: u64,
            live_bytes: u64,
        }

        let Some(callers) = &snapshot.callers else {
            return Self {
                thread_count: 0,
                total_events: 0,
                retained_events: 0,
                lost_events: 0,
                hotspots: Vec::new(),
            };
        };
        let mut totals = HashMap::<Vec<u64>, Total>::new();
        let mut live = HashMap::<(u64, u64), (Vec<u64>, u64)>::new();
        for event in &callers.events {
            if event.kind == EventKind::Allocated {
                let total = totals.entry(event.call_stack.clone()).or_default();
                total.allocations = total.allocations.saturating_add(1);
                total.allocated_bytes = total.allocated_bytes.saturating_add(event.size);
                live.insert((event.thread_log_id, event.allocation_id), (event.call_stack.clone(), event.size));
            } else if event.kind == EventKind::Deallocated {
                live.remove(&(event.thread_log_id, event.allocation_id));
            }
        }
        for (_, (stack, size)) in live {
            let total = totals.entry(stack).or_default();
            total.live_allocations = total.live_allocations.saturating_add(1);
            total.live_bytes = total.live_bytes.saturating_add(size);
        }
        let lookups = snapshot
            .addresses
            .iter()
            .map(|lookup| (lookup.address, lookup))
            .collect::<HashMap<_, _>>();
        let mut hotspots = totals
            .into_iter()
            .map(|(stack, total)| {
                let application_stack = hotspot_stack(&stack, &lookups, AllocationStackFilter::Application);
                let complete_stack = hotspot_stack(&stack, &lookups, AllocationStackFilter::All);
                AllocationHotspot {
                    allocations: total.allocations,
                    allocated_bytes: total.allocated_bytes,
                    live_allocations: total.live_allocations,
                    live_bytes: total.live_bytes,
                    application_stack,
                    complete_stack,
                }
            })
            .collect::<Vec<_>>();
        hotspots.sort_unstable_by(|left, right| {
            right
                .allocations
                .cmp(&left.allocations)
                .then_with(|| right.allocated_bytes.cmp(&left.allocated_bytes))
                .then_with(|| right.live_bytes.cmp(&left.live_bytes))
        });
        Self {
            thread_count: u64::try_from(callers.threads.len()).unwrap_or(u64::MAX),
            total_events: callers.total_events,
            retained_events: u64::try_from(callers.events.len()).unwrap_or(u64::MAX),
            lost_events: callers.lost_events,
            hotspots,
        }
    }

    pub(super) fn sorted_hotspots(&self, sort: AllocationSort, descending: bool) -> Vec<&AllocationHotspot> {
        let mut hotspots = self.hotspots.iter().collect::<Vec<_>>();
        hotspots.sort_unstable_by(|left, right| {
            let ordering = match sort {
                AllocationSort::Allocations => left.allocations.cmp(&right.allocations),
                AllocationSort::AllocatedBytes => left.allocated_bytes.cmp(&right.allocated_bytes),
                AllocationSort::AverageBytes => average_bytes(left).cmp(&average_bytes(right)),
                AllocationSort::LiveAllocations => left.live_allocations.cmp(&right.live_allocations),
                AllocationSort::LiveBytes => left.live_bytes.cmp(&right.live_bytes),
            }
            .then_with(|| left.allocated_bytes.cmp(&right.allocated_bytes))
            .then_with(|| left.allocations.cmp(&right.allocations));
            if descending { ordering.reverse() } else { ordering }
        });
        hotspots
    }
}

fn average_bytes(hotspot: &AllocationHotspot) -> u64 {
    hotspot.allocated_bytes.checked_div(hotspot.allocations).unwrap_or_default()
}

fn hotspot_stack(
    stack: &[u64],
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    filter: AllocationStackFilter,
) -> Vec<String> {
    let frames = stack
        .iter()
        .filter_map(|address| {
            let lookup = lookups.get(address).copied();
            (filter == AllocationStackFilter::All || !is_internal_allocation_frame(lookup)).then(|| format_hotspot_frame(*address, lookup))
        })
        .collect::<Vec<_>>();
    if filter == AllocationStackFilter::Application && frames.is_empty() && !stack.is_empty() {
        vec!["No application frames after filtering".into()]
    } else {
        frames
    }
}

fn primitive_stack(
    stack: &[u64],
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    filter: AllocationStackFilter,
) -> Vec<String> {
    let frames = stack
        .iter()
        .filter_map(|address| {
            let lookup = lookups.get(address).copied();
            (filter == AllocationStackFilter::All || !is_internal_primitive_frame(lookup)).then(|| format_hotspot_frame(*address, lookup))
        })
        .collect::<Vec<_>>();
    if filter == AllocationStackFilter::Application && frames.is_empty() && !stack.is_empty() {
        vec!["No application frames after filtering".into()]
    } else {
        frames
    }
}

fn is_internal_primitive_frame(lookup: Option<&seismograph_rallocator::callers::AddressLookup>) -> bool {
    let Some(lookup) = lookup else {
        return false;
    };
    if lookup.symbol.as_deref().is_some_and(|symbol| {
        let symbol = symbol.trim_start_matches('<');
        symbol.starts_with("performables::")
            || symbol.starts_with("seismograph::")
            || symbol.starts_with("std::")
            || symbol.starts_with("alloc::")
            || symbol.starts_with("core::")
            || symbol.starts_with("backtrace::")
    }) {
        return true;
    }
    lookup.filename.as_deref().is_some_and(|filename| {
        let filename = filename.replace('\\', "/").to_ascii_lowercase();
        filename.contains("/performables/")
            || filename.contains("/seismograph/")
            || filename.contains("/library/std/src/")
            || filename.contains("/library/alloc/src/")
            || filename.contains("/library/core/src/")
    })
}

fn is_internal_allocation_frame(lookup: Option<&seismograph_rallocator::callers::AddressLookup>) -> bool {
    let Some(lookup) = lookup else {
        return false;
    };
    if lookup.symbol.as_deref().is_some_and(|symbol| {
        let symbol = symbol.trim_start_matches('<');
        symbol.starts_with("rallocator::")
            || symbol.starts_with("seismograph::")
            || symbol.starts_with("seismograph_rallocator::")
            || symbol.starts_with("std::")
            || symbol.starts_with("alloc::")
            || symbol.starts_with("core::")
    }) {
        return true;
    }
    lookup.filename.as_deref().is_some_and(|filename| {
        let filename = filename.replace('\\', "/").to_ascii_lowercase();
        filename.contains("/rallocator/")
            || filename.contains("/seismograph/")
            || filename.contains("/seismograph_rallocator/")
            || filename.contains("/library/std/src/")
            || filename.contains("/library/alloc/src/")
            || filename.contains("/library/core/src/")
    })
}

fn format_hotspot_frame(address: u64, lookup: Option<&seismograph_rallocator::callers::AddressLookup>) -> String {
    let Some(lookup) = lookup else {
        return format!("0x{address:016x}");
    };
    let symbol = lookup.symbol.as_deref().unwrap_or("unknown");
    let Some(filename) = &lookup.filename else {
        return symbol.to_owned();
    };
    let filename = Path::new(filename).file_name().and_then(|name| name.to_str()).unwrap_or(filename);
    match lookup.line {
        Some(line) => format!("{symbol} ({filename}:{line})"),
        None => format!("{symbol} ({filename})"),
    }
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::RecordingPolicies;
    use seismograph::recorder::event::{
        Address as RuntimeAddress, Event as RuntimeEvent, EventClock, EventKind as RuntimeEventKind, EventPayload, EventSequence,
        EventTimestamp, Events, ObjectId,
    };
    use seismograph::recorder::runtime::{RuntimeEvent as RuntimeEventPayload, RuntimeId, WorkerId};
    use seismograph::recorder::thread::{ThreadId, ThreadLog};
    use seismograph_rallocator::callers::{
        AddressLookup, AddressLookupFields, Callers, CallersFields, Event, EventFields, EventKind, HeapKind,
    };

    use super::*;

    #[test]
    fn model_enum_navigation_and_labels_cover_every_variant() {
        assert_eq!(
            PrimitiveKind::ALL.map(|kind| (kind.label(), kind.operations().len())),
            [
                ("Arc", 5),
                ("Mutex", 6),
                ("RwLock", 9),
                ("Barrier", 3),
                ("Condvar", 3),
                ("OnceLock / LazyLock", 3),
                ("Channel", 6),
            ]
        );
        assert_eq!(
            [MemoryTier::Small, MemoryTier::Medium, MemoryTier::Direct,].map(|tier| (
                tier.index(),
                tier.label(),
                tier.next(),
                tier.previous()
            )),
            [
                (0, "Small", MemoryTier::Medium, MemoryTier::Direct),
                (1, "Medium", MemoryTier::Direct, MemoryTier::Small),
                (2, "Direct", MemoryTier::Small, MemoryTier::Medium),
            ]
        );
        assert_eq!(
            [
                AllocationSort::Allocations,
                AllocationSort::AllocatedBytes,
                AllocationSort::AverageBytes,
                AllocationSort::LiveAllocations,
                AllocationSort::LiveBytes,
            ]
            .map(|sort| (sort.next(), sort.previous())),
            [
                (AllocationSort::AllocatedBytes, AllocationSort::LiveBytes),
                (AllocationSort::AverageBytes, AllocationSort::Allocations),
                (AllocationSort::LiveAllocations, AllocationSort::AllocatedBytes),
                (AllocationSort::LiveBytes, AllocationSort::AverageBytes),
                (AllocationSort::Allocations, AllocationSort::LiveAllocations),
            ]
        );
        assert_eq!(
            [
                PrimitiveSort::Events,
                PrimitiveSort::Objects,
                PrimitiveSort::Threads,
                PrimitiveSort::Hotspots,
            ]
            .map(|sort| (sort.next(), sort.previous())),
            [
                (PrimitiveSort::Objects, PrimitiveSort::Hotspots),
                (PrimitiveSort::Threads, PrimitiveSort::Events),
                (PrimitiveSort::Hotspots, PrimitiveSort::Objects),
                (PrimitiveSort::Events, PrimitiveSort::Threads),
            ]
        );
        assert_eq!(
            [
                RuntimeTaskSort::Task,
                RuntimeTaskSort::Polls,
                RuntimeTaskSort::AveragePoll,
                RuntimeTaskSort::MaximumPoll,
                RuntimeTaskSort::AverageResume,
                RuntimeTaskSort::MaximumResume,
                RuntimeTaskSort::AverageReadyWait,
                RuntimeTaskSort::MaximumReadyWait,
            ]
            .map(|sort| (sort.next(), sort.previous(), sort.label())),
            [
                (RuntimeTaskSort::Polls, RuntimeTaskSort::MaximumReadyWait, "task"),
                (RuntimeTaskSort::AveragePoll, RuntimeTaskSort::Task, "polls"),
                (RuntimeTaskSort::MaximumPoll, RuntimeTaskSort::Polls, "average poll"),
                (RuntimeTaskSort::AverageResume, RuntimeTaskSort::AveragePoll, "maximum poll"),
                (RuntimeTaskSort::MaximumResume, RuntimeTaskSort::MaximumPoll, "average resume"),
                (RuntimeTaskSort::AverageReadyWait, RuntimeTaskSort::AverageResume, "maximum resume"),
                (RuntimeTaskSort::MaximumReadyWait, RuntimeTaskSort::MaximumResume, "average stall"),
                (RuntimeTaskSort::Task, RuntimeTaskSort::AverageReadyWait, "maximum stall"),
            ]
        );
        assert_eq!(
            [
                RuntimeTaskMetricScope::Lifetime.label(),
                RuntimeTaskMetricScope::RetainedWindow.label(),
            ],
            ["lifetime", "retained window"]
        );
        assert_eq!(
            (AllocationStackFilter::Application.toggle(), AllocationStackFilter::All.toggle()),
            (AllocationStackFilter::All, AllocationStackFilter::Application)
        );
    }

    #[test]
    fn primitive_operation_metadata_covers_every_variant() {
        let operations = [
            PrimitiveOperationKind::ArcCreate,
            PrimitiveOperationKind::ArcClone,
            PrimitiveOperationKind::ArcDeref,
            PrimitiveOperationKind::ArcDrop,
            PrimitiveOperationKind::ArcRelocate,
            PrimitiveOperationKind::MutexAccess,
            PrimitiveOperationKind::MutexContention,
            PrimitiveOperationKind::MutexRelease,
            PrimitiveOperationKind::RwLockReadAccess,
            PrimitiveOperationKind::RwLockReadContention,
            PrimitiveOperationKind::RwLockReadRelease,
            PrimitiveOperationKind::RwLockWriteAccess,
            PrimitiveOperationKind::RwLockWriteContention,
            PrimitiveOperationKind::RwLockWriteRelease,
            PrimitiveOperationKind::BarrierAccess,
            PrimitiveOperationKind::BarrierContention,
            PrimitiveOperationKind::BarrierRelease,
            PrimitiveOperationKind::CondvarAccess,
            PrimitiveOperationKind::CondvarContention,
            PrimitiveOperationKind::CondvarNotify,
            PrimitiveOperationKind::OnceAccess,
            PrimitiveOperationKind::OnceContention,
            PrimitiveOperationKind::OnceInitialize,
            PrimitiveOperationKind::ChannelSend,
            PrimitiveOperationKind::ChannelSendContention,
            PrimitiveOperationKind::ChannelReceive,
            PrimitiveOperationKind::ChannelReceiveContention,
            PrimitiveOperationKind::ChannelClose,
            PrimitiveOperationKind::ChannelHighWatermark,
            PrimitiveOperationKind::LockPoisoned,
            PrimitiveOperationKind::LockPoisonObserved,
            PrimitiveOperationKind::LockPoisonCleared,
        ];

        assert_eq!(
            operations
                .map(|operation| (
                    operation.label(),
                    operation.event_kind(),
                    operation.is_lock_poison(),
                    operation.is_contention()
                ))
                .len(),
            32
        );
        for kind in PrimitiveKind::ALL {
            for operation in operations {
                let _ = kind.identifies(operation.event_kind());
            }
        }
    }

    #[test]
    fn thread_operation_metadata_covers_every_variant() {
        let event_kinds = ThreadOperationKind::ALL.map(ThreadOperationKind::event_kind);
        for operation in ThreadOperationKind::ALL {
            assert!(!operation.label().is_empty());
            assert!(!operation.relationship_label().is_empty());
            let _ = operation.is_contention();
            let _ = operation.is_allocation();
            for event_kind in event_kinds {
                let _ = operation.is_related(event_kind);
            }
        }
    }

    #[test]
    fn stack_formatting_filters_internal_frames_and_handles_missing_metadata() {
        let lookups = [
            AddressLookup::from_fields(AddressLookupFields {
                address: 1,
                symbol: Some("<std::alloc::allocate>".into()),
                filename: None,
                line: None,
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 2,
                symbol: Some("app::run".into()),
                filename: Some(r"C:\src\app.rs".into()),
                line: Some(7),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 3,
                symbol: None,
                filename: Some(r"C:\src\file.rs".into()),
                line: None,
                column: None,
            }),
        ];
        let lookups = lookups.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();

        assert_eq!(
            (
                hotspot_stack(&[1, 2], &lookups, AllocationStackFilter::Application),
                primitive_stack(&[1], &lookups, AllocationStackFilter::Application),
                hotspot_stack(&[1], &lookups, AllocationStackFilter::Application),
                hotspot_stack(&[4], &lookups, AllocationStackFilter::Application),
                format_hotspot_frame(3, lookups.get(&3).copied()),
                format_hotspot_frame(4, None),
            ),
            (
                vec!["app::run (app.rs:7)".to_owned()],
                vec!["No application frames after filtering".to_owned()],
                vec!["No application frames after filtering".to_owned()],
                vec!["0x0000000000000004".to_owned()],
                "unknown (file.rs)".to_owned(),
                "0x0000000000000004".to_owned(),
            )
        );
    }

    #[test]
    fn allocation_routing_and_histogram_bounds_cover_edges() {
        assert_eq!(
            (
                allocation_tier(1, 1, 64, 4_096, 65_536, 1 << 30),
                allocation_tier(65, 1, 64, 4_096, 65_536, 1 << 30),
                allocation_tier(1, 65_537, 64, 4_096, 65_536, 1 << 30),
                histogram_bucket(0),
                histogram_bucket(1),
                histogram_bounds(0),
                histogram_bounds(1),
                histogram_bounds(64),
            ),
            (
                MemoryTier::Small,
                MemoryTier::Medium,
                MemoryTier::Direct,
                0,
                1,
                (0, 0),
                (1, 1),
                (1 << 63, u64::MAX),
            )
        );
    }

    fn runtime_task(task_id: u64) -> RuntimeTaskSummary {
        RuntimeTaskSummary {
            task_id,
            runtime_id: 1,
            parent_id: None,
            type_descriptor_id: None,
            metric_scope: RuntimeTaskMetricScope::RetainedWindow,
            state: "Pending".into(),
            spawned_at: None,
            completed_at: None,
            poll_count: task_id,
            poll_nanos: task_id * 10,
            average_poll_nanos: task_id * 2,
            max_poll_nanos: task_id * 3,
            resume_count: task_id,
            average_resume_nanos: task_id * 4,
            max_resume_nanos: task_id * 5,
            ready_wait_count: task_id,
            ready_wait_nanos: task_id * 6,
            average_ready_wait_nanos: task_id * 7,
            max_ready_wait_nanos: task_id * 8,
            enqueue_count: 0,
            materialization_count: 0,
            transfer_count: 0,
            worker_ids: Vec::new(),
            spawn_stack: Vec::new(),
        }
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one table-driven test covers every sort key and both directions")]
    fn sorting_helpers_cover_every_column_and_direction() {
        let worker = RuntimeWorkerSummary {
            runtime_id: 1,
            runtime_name: String::new(),
            worker_id: 1,
            role: String::new(),
            state: String::new(),
            thread_id: None,
            current_task: None,
            average_running_tasks: 0.0,
            poll_count: 0,
            average_poll_nanos: 0,
            max_poll_nanos: 0,
            tasks: vec![runtime_task(1), runtime_task(2)],
        };
        for sort in [
            RuntimeTaskSort::Task,
            RuntimeTaskSort::Polls,
            RuntimeTaskSort::AveragePoll,
            RuntimeTaskSort::MaximumPoll,
            RuntimeTaskSort::AverageResume,
            RuntimeTaskSort::MaximumResume,
            RuntimeTaskSort::AverageReadyWait,
            RuntimeTaskSort::MaximumReadyWait,
        ] {
            assert_eq!(
                worker
                    .sorted_tasks(sort, false)
                    .into_iter()
                    .map(|task| task.task_id)
                    .collect::<Vec<_>>(),
                vec![1, 2]
            );
            assert_eq!(
                worker
                    .sorted_tasks(sort, true)
                    .into_iter()
                    .map(|task| task.task_id)
                    .collect::<Vec<_>>(),
                vec![2, 1]
            );
        }

        let operations = PrimitiveGroup {
            kind: PrimitiveKind::Arc,
            events: 0,
            objects: 0,
            contentions: 0,
            operations: vec![
                PrimitiveOperation {
                    kind: PrimitiveOperationKind::ArcClone,
                    events: 1,
                    objects: 2,
                    threads: 3,
                    hotspots: vec![PrimitiveHotspot {
                        count: 1,
                        application_stack: Vec::new(),
                        complete_stack: Vec::new(),
                    }],
                },
                PrimitiveOperation {
                    kind: PrimitiveOperationKind::ArcDeref,
                    events: 2,
                    objects: 3,
                    threads: 4,
                    hotspots: vec![
                        PrimitiveHotspot {
                            count: 1,
                            application_stack: Vec::new(),
                            complete_stack: Vec::new(),
                        },
                        PrimitiveHotspot {
                            count: 1,
                            application_stack: Vec::new(),
                            complete_stack: Vec::new(),
                        },
                    ],
                },
            ],
        };
        for sort in [
            PrimitiveSort::Events,
            PrimitiveSort::Objects,
            PrimitiveSort::Threads,
            PrimitiveSort::Hotspots,
        ] {
            assert_eq!(operations.sorted_operations(sort, false)[0].kind, PrimitiveOperationKind::ArcClone);
            assert_eq!(operations.sorted_operations(sort, true)[0].kind, PrimitiveOperationKind::ArcDeref);
        }

        let hotspot = |allocations, allocated_bytes, live_allocations, live_bytes| AllocationHotspot {
            allocations,
            allocated_bytes,
            live_allocations,
            live_bytes,
            application_stack: Vec::new(),
            complete_stack: Vec::new(),
        };
        let allocations = AllocationSnapshot {
            thread_count: 0,
            total_events: 0,
            retained_events: 0,
            lost_events: 0,
            hotspots: vec![hotspot(1, 10, 1, 10), hotspot(2, 40, 2, 40)],
        };
        for sort in [
            AllocationSort::Allocations,
            AllocationSort::AllocatedBytes,
            AllocationSort::AverageBytes,
            AllocationSort::LiveAllocations,
            AllocationSort::LiveBytes,
        ] {
            assert_eq!(allocations.sorted_hotspots(sort, false)[0].allocations, 1);
            assert_eq!(allocations.sorted_hotspots(sort, true)[0].allocations, 2);
        }
        assert_eq!(average_bytes(&hotspot(0, 10, 0, 0)), 0);
    }

    #[test]
    fn snapshot_accessors_cover_empty_and_populated_stacks() {
        let allocation = AllocationHotspot {
            allocations: 1,
            allocated_bytes: 1,
            live_allocations: 1,
            live_bytes: 1,
            application_stack: vec!["app".into()],
            complete_stack: vec!["internal".into(), "app".into()],
        };
        let primitive = PrimitiveHotspot {
            count: 1,
            application_stack: Vec::new(),
            complete_stack: vec!["internal".into()],
        };
        let object = ThreadObject {
            object_id: 1,
            selected_events: 2,
            related_events: 3,
            selected_stacks: vec![ThreadStack {
                count: 1,
                application_stack: vec!["selected".into()],
                complete_stack: vec!["selected-all".into()],
            }],
            related_stacks: Vec::new(),
        };

        assert_eq!(
            (
                allocation.location(AllocationStackFilter::Application),
                allocation.stack(AllocationStackFilter::All),
                primitive.location(AllocationStackFilter::Application),
                primitive.stack(AllocationStackFilter::All),
                object.hotness(),
                object.selected_stack().unwrap().stack(AllocationStackFilter::Application),
                object.related_stack(),
            ),
            (
                "app",
                &["internal".to_owned(), "app".to_owned()][..],
                "Backtraces disabled",
                &["internal".to_owned()][..],
                5,
                &["selected".to_owned()][..],
                None,
            )
        );
    }

    #[test]
    fn retained_memory_totals_and_task_ids_handle_missing_inputs() {
        let snapshot = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(1, 0, 0));
        assert!(retained_memory_totals(&snapshot, &[]).is_empty());
        assert_eq!(
            (
                runtime_task_id(RuntimeEventKind::TaskSpawned, 1, 0),
                runtime_task_id(RuntimeEventKind::TaskPollFinished, 2, 0),
                runtime_task_id(RuntimeEventKind::TransferStarted, 0, 3),
                runtime_task_id(RuntimeEventKind::InstanceRelocated, 0, 4),
                runtime_task_id(RuntimeEventKind::TransferFinished, 0, 5),
                runtime_task_id(RuntimeEventKind::ArcClone, 6, 7),
                runtime_task_id(RuntimeEventKind::TaskCanceled, 0, 0),
            ),
            (Some(1), Some(2), Some(3), Some(4), Some(5), None, None)
        );
    }

    #[test]
    fn runtime_task_builder_handles_zero_and_populated_metrics() {
        let empty = RuntimeTaskSummary::from_builder(1, &RuntimeTaskBuilder::default());
        let builder = RuntimeTaskBuilder {
            runtime_id: 2,
            parent_id: Some(3),
            type_descriptor_id: Some(4),
            metric_scope: RuntimeTaskMetricScope::Lifetime,
            state: "Completed".into(),
            spawned_at: Some(5),
            completed_at: Some(6),
            poll_count: 2,
            poll_nanos: 20,
            max_poll_nanos: 15,
            last_poll_finished_at: None,
            resume_count: 4,
            resume_nanos: 40,
            max_resume_nanos: 20,
            ready_wait_count: 5,
            ready_wait_nanos: 50,
            max_ready_wait_nanos: 30,
            enqueue_count: 6,
            materialization_count: 7,
            transfer_count: 8,
            worker_ids: HashSet::from([9]),
            spawn_stack: vec!["spawn".into()],
        };
        let populated = RuntimeTaskSummary::from_builder(10, &builder);

        assert_eq!(
            (
                empty.average_poll_nanos,
                empty.average_resume_nanos,
                empty.average_ready_wait_nanos,
                populated.task_id,
                populated.average_poll_nanos,
                populated.average_resume_nanos,
                populated.average_ready_wait_nanos,
                populated.worker_ids,
            ),
            (0, 0, 0, 10, 10, 10, 10, vec![9])
        );
    }

    #[test]
    fn lifetime_task_ignores_retained_spawn_event() {
        let mut task = RuntimeTaskBuilder {
            metric_scope: RuntimeTaskMetricScope::Lifetime,
            ..RuntimeTaskBuilder::default()
        };
        let event = RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(1),
            kind: RuntimeEventKind::TaskSpawned,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: None,
                subject_id: 1,
                related_id: 0,
                value_0: 0,
                value_1: 0,
            }),
            call_stack: Vec::new(),
        };

        record_task_spawn(&mut task, 1, 1, &event, &HashMap::new());

        assert_eq!(task.state, "");
    }

    #[test]
    fn allocation_snapshot_without_callers_and_unmatched_deallocation_is_empty() {
        let mut snapshot = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(1, 0, 0));
        assert_eq!(
            AllocationSnapshot::from_snapshot(&snapshot),
            AllocationSnapshot {
                thread_count: 0,
                total_events: 0,
                retained_events: 0,
                lost_events: 0,
                hotspots: Vec::new(),
            }
        );

        let mut event = Event::default();
        event.kind = EventKind::Deallocated;
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 0,
            total_events: 1,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![event],
            thread_names: Vec::new(),
        }));
        assert!(AllocationSnapshot::from_snapshot(&snapshot).hotspots.is_empty());
    }

    #[test]
    fn memory_bucket_ranks_equal_counts_by_allocated_bytes() {
        let mut total = MemoryBucketTotal::default();
        total.hotspots.insert(
            vec![1],
            MemoryHotspotTotal {
                allocations: 1,
                allocated_bytes: 10,
                ..MemoryHotspotTotal::default()
            },
        );
        total.hotspots.insert(
            vec![2],
            MemoryHotspotTotal {
                allocations: 1,
                allocated_bytes: 20,
                ..MemoryHotspotTotal::default()
            },
        );

        assert_eq!(memory_bucket(1, 2, total, &HashMap::new()).hotspots[0].allocated_bytes, 20);
    }

    #[test]
    fn runtime_monitor_applies_lifecycle_and_transfer_events() {
        let event = |sequence, kind, subject_id, related_id| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: Some(WorkerId::from_raw(1).unwrap()),
                subject_id,
                related_id,
                value_0: 0,
                value_1: 0,
            }),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, RuntimeEventKind::TaskSpawned, 1, 0),
            event(2, RuntimeEventKind::TaskEnqueued, 1, 0),
            event(3, RuntimeEventKind::TaskMaterialized, 1, 0),
            event(4, RuntimeEventKind::TransferStarted, 0, 1),
            event(5, RuntimeEventKind::InstanceRelocated, 0, 1),
            event(6, RuntimeEventKind::TransferFinished, 0, 1),
            event(7, RuntimeEventKind::TaskCanceled, 1, 0),
            event(8, RuntimeEventKind::TaskSpawned, 2, 0),
            event(9, RuntimeEventKind::TaskPanicked, 2, 0),
            event(10, RuntimeEventKind::TaskSpawned, 3, 0),
            event(11, RuntimeEventKind::TaskCompleted, 3, 0),
            event(12, RuntimeEventKind::RuntimeCreated, 0, 0),
            RuntimeEvent {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(13),
                timestamp: EventTimestamp::from_ticks(13),
                kind: RuntimeEventKind::ArcClone,
                payload: EventPayload::Object(ObjectId::new(9)),
                call_stack: Vec::new(),
            },
        ];

        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                clock: EventClock::Unspecified,
                total_events: 13,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: Vec::new(),
                events,
            },
            None,
            &[],
        );
        let tasks = &snapshot.workers[0].tasks;

        assert_eq!(
            tasks
                .iter()
                .map(|task| (task.task_id, task.state.as_str(), task.transfer_count))
                .collect::<Vec<_>>(),
            vec![(1, "Canceled", 3), (2, "Panicked", 0), (3, "Completed", 0)]
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the single assertion compares the complete memory snapshot fixture"
    )]
    fn memory_snapshot_summarizes_regions_and_slices() {
        let mut snapshot = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(0, 1, 0));
        snapshot.stats.live_bytes = 10;
        snapshot.stats.peak_live_bytes = 20;
        snapshot.stats.mapped_bytes = 30;
        snapshot.stats.allocations = 40;
        let mut region = seismograph_rallocator::snapshot::Region::default();
        region.reserved_bytes = 1_024;
        region.used_slices = 3;
        region.free_slices = 13;
        snapshot.regions.push(region);
        let mut topology = seismograph_rallocator::topology::TopologyRegion::default();
        topology.slice_bytes = 64 * 1024;
        for kind in [
            seismograph_rallocator::topology::SliceKind::Small,
            seismograph_rallocator::topology::SliceKind::Medium,
            seismograph_rallocator::topology::SliceKind::MediumContinuation,
            seismograph_rallocator::topology::SliceKind::Bump,
        ] {
            let mut slice = seismograph_rallocator::topology::Slice::default();
            slice.kind = kind;
            if kind == seismograph_rallocator::topology::SliceKind::Small {
                slice.segments.push(seismograph_rallocator::topology::Segment::from_fields(
                    seismograph_rallocator::topology::SegmentFields {
                        segment_index: 0,
                        class_index: 2,
                        context: false,
                        live_blocks: 0,
                        usable_blocks: 100,
                        utilization_tracked: false,
                    },
                ));
            }
            if kind == seismograph_rallocator::topology::SliceKind::Medium {
                slice.span_slices = 2;
                slice.requested_bytes = 80_000;
                slice.usable_bytes = 96_000;
            }
            topology.slices.push(slice);
        }
        snapshot.topology.push(topology);
        snapshot.size_classes.push(seismograph_rallocator::snapshot::SizeClass::from_fields(
            seismograph_rallocator::snapshot::SizeClassFields {
                class_index: 2,
                block_bytes: 64,
                live_allocations: seismograph_rallocator::snapshot::Estimate::from_fields(
                    seismograph_rallocator::snapshot::EstimateFields {
                        value: 25,
                        lower_bound: 24,
                        upper_bound: 26,
                    },
                ),
                requested_bytes: seismograph_rallocator::snapshot::Estimate::from_fields(
                    seismograph_rallocator::snapshot::EstimateFields {
                        value: 1_200,
                        lower_bound: 1_100,
                        upper_bound: 1_300,
                    },
                ),
                usable_bytes: seismograph_rallocator::snapshot::Estimate::from_fields(seismograph_rallocator::snapshot::EstimateFields {
                    value: 1_600,
                    lower_bound: 1_536,
                    upper_bound: 1_664,
                }),
            },
        ));

        assert_eq!(
            MemorySnapshot::from_snapshot(&snapshot),
            MemorySnapshot {
                live_bytes: 10,
                peak_live_bytes: 20,
                mapped_bytes: 30,
                allocations: 40,
                reserved_bytes: 1_024,
                used_slices: 3,
                free_slices: 13,
                slice_bytes: 64 * 1024,
                small_slices: 1,
                medium_slices: 2,
                bump_slices: 1,
                unknown_slices: 0,
                regions: vec![MemoryRegion {
                    index: 0,
                    reserved_bytes: 1_024,
                    used_slices: 3,
                    free_slices: 13,
                }],
                size_classes: vec![MemorySizeClass {
                    block_bytes: 64,
                    live_allocations: 25,
                    capacity_blocks: 100,
                    requested_bytes: 1_200,
                    usable_bytes: 1_600,
                }],
                medium_allocations: MediumAllocations {
                    count: 1,
                    requested_bytes: 80_000,
                    usable_bytes: 96_000,
                    span_slices: 2,
                    largest_requested_bytes: 80_000,
                },
                tiers: vec![
                    MemoryTierData {
                        kind: MemoryTier::Small,
                        current_allocations: 25,
                        current_bytes: 1_200,
                        buckets: vec![MemoryBucket {
                            lower_bytes: 1,
                            upper_bytes: 64,
                            allocations: 0,
                            allocated_bytes: 0,
                            live_allocations: 0,
                            live_bytes: 0,
                            topology_live_allocations: Some(25),
                            capacity_blocks: Some(100),
                            requested_bytes: Some(1_200),
                            usable_bytes: Some(1_600),
                            hotspots: Vec::new(),
                        }],
                    },
                    MemoryTierData {
                        kind: MemoryTier::Medium,
                        current_allocations: 1,
                        current_bytes: 80_000,
                        buckets: Vec::new(),
                    },
                    MemoryTierData {
                        kind: MemoryTier::Direct,
                        current_allocations: 0,
                        current_bytes: 0,
                        buckets: Vec::new(),
                    },
                ],
            }
        );
    }

    #[test]
    fn memory_snapshot_groups_retained_allocations_by_routing_shape() {
        let mut snapshot = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(0, 1, 0));
        snapshot.size_classes.push(seismograph_rallocator::snapshot::SizeClass::from_fields(
            seismograph_rallocator::snapshot::SizeClassFields {
                class_index: 0,
                block_bytes: 64,
                live_allocations: seismograph_rallocator::snapshot::Estimate::default(),
                requested_bytes: seismograph_rallocator::snapshot::Estimate::default(),
                usable_bytes: seismograph_rallocator::snapshot::Estimate::default(),
            },
        ));
        let event = |allocation_id, size, align, address| {
            Event::from_fields(EventFields {
                thread_log_id: 1,
                event_thread_id: 1,
                sequence: allocation_id,
                allocation_id,
                kind: EventKind::Allocated,
                heap_id: 1,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: allocation_id * 16,
                size,
                align,
                call_stack: vec![address],
            })
        };
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 1,
            total_events: 3,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![
                event(1, 32, 8, 0x1000),
                event(2, 100_000, 8, 0x2000),
                event(3, 32, 128 * 1024, 0x3000),
            ],
            thread_names: Vec::new(),
        }));
        snapshot.addresses = [("app::small", 0x1000), ("app::medium", 0x2000), ("app::direct", 0x3000)]
            .into_iter()
            .map(|(symbol, address)| {
                AddressLookup::from_fields(AddressLookupFields {
                    address,
                    symbol: Some(symbol.into()),
                    filename: None,
                    line: None,
                    column: None,
                })
            })
            .collect();

        let memory = MemorySnapshot::from_snapshot(&snapshot);

        assert_eq!(
            memory
                .tiers
                .iter()
                .map(|tier| {
                    (
                        tier.kind,
                        tier.buckets
                            .iter()
                            .map(|bucket| {
                                (
                                    bucket.lower_bytes,
                                    bucket.upper_bytes,
                                    bucket.allocations,
                                    bucket.hotspots[0].location(AllocationStackFilter::Application),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (MemoryTier::Small, vec![(1, 64, 1, "app::small")]),
                (MemoryTier::Medium, vec![(65_536, 131_071, 1, "app::medium")]),
                (MemoryTier::Direct, vec![(32, 63, 1, "app::direct")]),
            ]
        );
    }

    #[test]
    fn allocation_snapshot_ranks_hotspots_by_count_then_bytes() {
        let mut snapshot = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(0, 1, 0));
        let event = |allocation_id, kind, size, call_stack| {
            Event::from_fields(EventFields {
                thread_log_id: 1,
                event_thread_id: 1,
                sequence: allocation_id,
                allocation_id,
                kind,
                heap_id: 1,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: allocation_id * 16,
                size,
                align: 8,
                call_stack,
            })
        };
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 7,
            total_events: 4,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![
                event(1, EventKind::Allocated, 64, vec![0x3000, 0x1000]),
                event(2, EventKind::Allocated, 32, vec![0x3000, 0x1000]),
                event(1, EventKind::Deallocated, 64, Vec::new()),
                event(3, EventKind::Allocated, 4_096, vec![0x2000]),
            ],
            thread_names: Vec::new(),
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x1000,
            symbol: Some("app::small_allocations".into()),
            filename: Some(r"C:\src\app.rs".into()),
            line: Some(42),
            column: None,
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x2000,
            symbol: Some("app::large_allocation".into()),
            filename: Some(r"C:\src\large.rs".into()),
            line: Some(9),
            column: None,
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x3000,
            symbol: Some("rallocator::allocator::allocate".into()),
            filename: Some(r"C:\src\rallocator\src\allocator.rs".into()),
            line: Some(100),
            column: None,
        }));

        assert_eq!(
            AllocationSnapshot::from_snapshot(&snapshot),
            AllocationSnapshot {
                thread_count: 0,
                total_events: 4,
                retained_events: 4,
                lost_events: 0,
                hotspots: vec![
                    AllocationHotspot {
                        allocations: 2,
                        allocated_bytes: 96,
                        live_allocations: 1,
                        live_bytes: 32,
                        application_stack: vec!["app::small_allocations (app.rs:42)".into()],
                        complete_stack: vec![
                            "rallocator::allocator::allocate (allocator.rs:100)".into(),
                            "app::small_allocations (app.rs:42)".into(),
                        ],
                    },
                    AllocationHotspot {
                        allocations: 1,
                        allocated_bytes: 4_096,
                        live_allocations: 1,
                        live_bytes: 4_096,
                        application_stack: vec!["app::large_allocation (large.rs:9)".into()],
                        complete_stack: vec!["app::large_allocation (large.rs:9)".into()],
                    },
                ],
            }
        );
    }

    #[test]
    fn primitive_snapshot_aggregates_operations_and_application_stacks() {
        let event = |thread, sequence, object, kind, stack: Vec<u64>| RuntimeEvent {
            thread_id: ThreadId::new(thread),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: stack.into_iter().map(RuntimeAddress::new).collect(),
        };
        let events = vec![
            event(1, 1, 42, RuntimeEventKind::ArcDeref, vec![0x3000, 0x1000]),
            event(2, 1, 42, RuntimeEventKind::ArcDeref, vec![0x3000, 0x1000]),
            event(1, 2, 43, RuntimeEventKind::ArcClone, vec![0x2000]),
        ];
        let addresses = vec![
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x1000,
                symbol: Some("app::read_shared".into()),
                filename: Some(r"C:\src\app.rs".into()),
                line: Some(10),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x2000,
                symbol: Some("app::clone_shared".into()),
                filename: Some(r"C:\src\app.rs".into()),
                line: Some(20),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x3000,
                symbol: Some("performables::arc::Arc::deref".into()),
                filename: Some(r"C:\src\performables\src\arc\mod.rs".into()),
                line: Some(708),
                column: None,
            }),
        ];

        let snapshot = PrimitiveSnapshot::from_events(3, 0, &events, &addresses);

        assert_eq!(
            snapshot.groups[0],
            PrimitiveGroup {
                kind: PrimitiveKind::Arc,
                events: 3,
                objects: 2,
                contentions: 0,
                operations: vec![
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcCreate,
                        events: 0,
                        objects: 0,
                        threads: 0,
                        hotspots: Vec::new(),
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcClone,
                        events: 1,
                        objects: 1,
                        threads: 1,
                        hotspots: vec![PrimitiveHotspot {
                            count: 1,
                            application_stack: vec!["app::clone_shared (app.rs:20)".into()],
                            complete_stack: vec!["app::clone_shared (app.rs:20)".into()],
                        }],
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcDeref,
                        events: 2,
                        objects: 1,
                        threads: 2,
                        hotspots: vec![PrimitiveHotspot {
                            count: 2,
                            application_stack: vec!["app::read_shared (app.rs:10)".into()],
                            complete_stack: vec![
                                "performables::arc::Arc::deref (mod.rs:708)".into(),
                                "app::read_shared (app.rs:10)".into(),
                            ],
                        }],
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcDrop,
                        events: 0,
                        objects: 0,
                        threads: 0,
                        hotspots: Vec::new(),
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcRelocate,
                        events: 0,
                        objects: 0,
                        threads: 0,
                        hotspots: Vec::new(),
                    },
                ],
            }
        );
    }

    #[test]
    fn primitive_snapshot_counts_contentions_by_type() {
        let event = |sequence, object, kind| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, 10, RuntimeEventKind::MutexContention),
            event(2, 10, RuntimeEventKind::MutexContention),
            event(3, 20, RuntimeEventKind::RwLockReadContention),
            event(4, 20, RuntimeEventKind::RwLockWriteContention),
            event(5, 20, RuntimeEventKind::RwLockReadAccess),
        ];

        let snapshot = PrimitiveSnapshot::from_events(5, 0, &events, &[]);

        assert_eq!(
            snapshot.groups.iter().map(|group| group.contentions).collect::<Vec<_>>(),
            vec![0, 2, 2, 0, 0, 0, 0]
        );
    }

    #[test]
    fn lock_poison_events_correlate_with_their_lock_object_group() {
        let event = |sequence, object, kind| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, 10, RuntimeEventKind::MutexAccess),
            event(2, 10, RuntimeEventKind::LockPoisoned),
            event(3, 20, RuntimeEventKind::RwLockWriteAccess),
            event(4, 20, RuntimeEventKind::LockPoisonObserved),
        ];

        let snapshot = PrimitiveSnapshot::from_events(4, 0, &events, &[]);
        let mutex = snapshot.groups.iter().find(|group| group.kind == PrimitiveKind::Mutex).unwrap();
        let rw_lock = snapshot.groups.iter().find(|group| group.kind == PrimitiveKind::RwLock).unwrap();
        let mutex_poisoned = mutex
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisoned)
            .unwrap();
        let mutex_observed = mutex
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisonObserved)
            .unwrap();
        let rw_poisoned = rw_lock
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisoned)
            .unwrap();
        let rw_observed = rw_lock
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisonObserved)
            .unwrap();

        assert_eq!(
            (
                mutex.objects,
                mutex_poisoned.events,
                mutex_observed.events,
                rw_lock.objects,
                rw_poisoned.events,
                rw_observed.events,
            ),
            (1, 1, 0, 1, 0, 1)
        );
    }

    #[test]
    fn runtime_snapshot_keeps_events_without_allocator_addresses() {
        let decoded = seismograph::snapshot::DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::ProcessMonotonic,
                total_events: 1,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(1),
                    total_events: 1,
                    lost_events: 0,
                    name: "worker".into(),
                }],
                events: vec![RuntimeEvent {
                    thread_id: ThreadId::new(1),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(1),
                    kind: RuntimeEventKind::ArcDeref,
                    payload: EventPayload::Object(ObjectId::new(7)),
                    call_stack: vec![RuntimeAddress::new(0x1000)],
                }],
            },
            sources: Vec::new(),
        };

        let runtime = RuntimeSnapshot::from_events(&decoded, &[], None);

        assert_eq!(
            (
                runtime
                    .primitives
                    .groups
                    .iter()
                    .flat_map(|group| &group.operations)
                    .map(|operation| operation.events)
                    .sum::<u64>(),
                runtime.threads.threads.len(),
            ),
            (1, 1)
        );
    }

    #[test]
    fn runtime_monitor_summarizes_worker_and_task_yield_intervals() {
        let event = |sequence, timestamp, kind, subject_id, value_0, value_1| RuntimeEvent {
            thread_id: ThreadId::new(7),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: (kind != RuntimeEventKind::TaskSpawned).then(|| WorkerId::from_raw(2).unwrap()),
                subject_id,
                related_id: 0,
                value_0,
                value_1,
            }),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, 50, RuntimeEventKind::TaskSpawned, 10, 42, 0),
            event(2, 100, RuntimeEventKind::TaskPollStarted, 10, 30, 1),
            event(3, 300, RuntimeEventKind::TaskPollFinished, 10, 200, 0),
            event(4, 500, RuntimeEventKind::TaskPollStarted, 10, 80, 1),
            event(5, 900, RuntimeEventKind::TaskPollFinished, 10, 400, 0),
        ];

        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                clock: EventClock::Unspecified,
                total_events: 5,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: Vec::new(),
                events,
            },
            None,
            &[],
        );
        let worker = &snapshot.workers[0];
        let task = &worker.tasks[0];

        assert_eq!(
            (
                snapshot.total_events,
                snapshot.retained_events,
                snapshot.lost_events,
                task.metric_scope,
                task.state.as_str(),
            ),
            (5, 5, 0, RuntimeTaskMetricScope::RetainedWindow, "Pending")
        );
        assert_eq!(
            (
                worker.worker_id,
                worker.poll_count,
                worker.average_poll_nanos,
                worker.max_poll_nanos,
                worker.average_running_tasks,
                task.task_id,
                task.type_descriptor_id,
                task.poll_count,
                task.poll_nanos,
                task.average_poll_nanos,
                task.max_poll_nanos,
            ),
            (2, 2, 300, 400, 0.75, 10, Some(42), 2, 600, 300, 400)
        );
        assert_eq!(
            (
                task.resume_count,
                task.average_resume_nanos,
                task.max_resume_nanos,
                task.ready_wait_count,
                task.ready_wait_nanos,
                task.average_ready_wait_nanos,
                task.max_ready_wait_nanos,
            ),
            (1, 200, 200, 2, 110, 55, 80)
        );

        let mut worker = worker.clone();
        let mut other = task.clone();
        other.task_id = 11;
        other.max_ready_wait_nanos = 20;
        worker.tasks.push(other);
        assert_eq!(
            worker
                .sorted_tasks(RuntimeTaskSort::MaximumReadyWait, true)
                .into_iter()
                .map(|task| task.task_id)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the lifetime-source fixture keeps worker, task, and retained-event metrics together"
    )]
    fn runtime_source_lifetime_metrics_override_retained_event_counts() {
        use seismograph_runtime::snapshot::{
            Counters, Runtime, RuntimeState, Snapshot as RuntimeSourceSnapshot, Task, TaskMetrics, Worker, WorkerState,
        };
        use seismograph_runtime::worker::WorkerRole;

        let events = Events {
            clock: EventClock::Unspecified,
            total_events: 1_000,
            lost_events: 998,
            recording: RecordingPolicies::default(),
            threads: Vec::new(),
            events: vec![
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(100),
                    kind: RuntimeEventKind::TaskPollStarted,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 99,
                        value_1: 1,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(2),
                    timestamp: EventTimestamp::from_ticks(200),
                    kind: RuntimeEventKind::TaskPollFinished,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 100,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
            ],
        };
        let source = RuntimeSourceSnapshot {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "runtime".into(),
                configured_workers: 1,
                lifecycle_backtraces: seismograph::recorder::event::BacktraceCapture::Never,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(1),
                retired_at: None,
                counters: Counters::default(),
                workers: vec![
                    Worker {
                        id: WorkerId::from_raw(2).unwrap(),
                        role: WorkerRole::Core,
                        state: WorkerState::Running,
                        processor_index: None,
                        thread_id: Some(ThreadId::new(7)),
                        current_task: Some(seismograph::recorder::runtime::TaskId::from_raw(10).unwrap()),
                    },
                    Worker {
                        id: WorkerId::from_raw(3).unwrap(),
                        role: WorkerRole::Blocking,
                        state: WorkerState::Parked,
                        processor_index: None,
                        thread_id: None,
                        current_task: None,
                    },
                ],
                tasks: vec![
                    Task {
                        id: seismograph::recorder::runtime::TaskId::from_raw(10).unwrap(),
                        parent: None,
                        type_descriptor: seismograph::recorder::runtime::TypeDescriptorId::from_raw(42).unwrap(),
                        spawned_at: EventTimestamp::from_ticks(5),
                        last_worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        metrics: TaskMetrics {
                            poll_count: 500,
                            poll_duration_nanos: 10_000,
                            max_poll_duration_nanos: 300,
                            resume_count: 499,
                            resume_duration_nanos: 20_000,
                            max_resume_duration_nanos: 400,
                            ready_wait_count: 450,
                            ready_wait_duration_nanos: 9_000,
                            max_ready_wait_duration_nanos: 200,
                        },
                        spawn_backtrace: Vec::new(),
                    },
                    Task {
                        id: seismograph::recorder::runtime::TaskId::from_raw(11).unwrap(),
                        parent: None,
                        type_descriptor: seismograph::recorder::runtime::TypeDescriptorId::from_raw(43).unwrap(),
                        spawned_at: EventTimestamp::from_ticks(0),
                        last_worker_id: None,
                        metrics: TaskMetrics::default(),
                        spawn_backtrace: Vec::new(),
                    },
                ],
            }],
            addresses: Vec::new(),
        };

        let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source), &[]);
        let task = &snapshot.workers[0].tasks[0];

        assert_eq!(
            (
                snapshot.retained_events,
                snapshot.lost_events,
                task.metric_scope,
                task.state.as_str(),
                task.poll_count,
                task.poll_nanos,
                task.max_poll_nanos,
                task.resume_count,
                task.ready_wait_count,
            ),
            (2, 998, RuntimeTaskMetricScope::Lifetime, "Running", 500, 10_000, 300, 499, 450,)
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the single assertion verifies the complete cross-thread activity fixture"
    )]
    fn thread_snapshot_links_cross_thread_object_activity() {
        let event = |thread, sequence, object, kind, address| RuntimeEvent {
            thread_id: ThreadId::new(thread),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: vec![RuntimeAddress::new(address)],
        };
        let decoded = seismograph::snapshot::DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::ProcessMonotonic,
                total_events: 8,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: vec![
                    ThreadLog {
                        thread_id: ThreadId::new(1),
                        total_events: 2,
                        lost_events: 0,
                        name: "producer".into(),
                    },
                    ThreadLog {
                        thread_id: ThreadId::new(2),
                        total_events: 2,
                        lost_events: 0,
                        name: "consumer".into(),
                    },
                    ThreadLog {
                        thread_id: ThreadId::new(3),
                        total_events: 2,
                        lost_events: 0,
                        name: "waiter".into(),
                    },
                ],
                events: vec![
                    event(1, 1, 100, RuntimeEventKind::Allocation, 0x1000),
                    event(2, 1, 100, RuntimeEventKind::Deallocation, 0x2000),
                    event(3, 1, 200, RuntimeEventKind::MutexContention, 0x3000),
                    event(2, 2, 200, RuntimeEventKind::MutexAccess, 0x4000),
                    event(1, 2, 300, RuntimeEventKind::ArcClone, 0x5000),
                    event(3, 2, 300, RuntimeEventKind::ArcDeref, 0x6000),
                    event(1, 3, 101, RuntimeEventKind::Allocation, 0x1000),
                    event(2, 3, 101, RuntimeEventKind::Deallocation, 0x2000),
                ],
            },
            sources: Vec::new(),
        };
        let addresses = vec![
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x1000,
                symbol: Some("app::allocate".into()),
                filename: Some("producer.rs".into()),
                line: Some(10),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x2000,
                symbol: Some("app::free".into()),
                filename: Some("consumer.rs".into()),
                line: Some(20),
                column: None,
            }),
        ];

        let snapshot = ThreadSnapshot::from_events(&decoded.events, &addresses);
        let operation = |thread: usize, kind| {
            snapshot.threads[thread]
                .operations
                .iter()
                .find(|operation| operation.kind == kind)
                .unwrap()
                .participants
                .first()
                .unwrap()
        };
        let allocation = operation(0, ThreadOperationKind::Allocation);
        let contention = operation(2, ThreadOperationKind::MutexContention);
        let arc = operation(0, ThreadOperationKind::ArcClone);
        let allocated_object = allocation.objects.iter().find(|object| object.object_id == 100).unwrap();

        assert_eq!(
            (
                snapshot.threads.iter().map(|thread| thread.thread_id).collect::<Vec<_>>(),
                (
                    allocation.thread_id,
                    allocated_object.object_id,
                    allocated_object.selected_events,
                    allocated_object.related_events,
                ),
                allocated_object
                    .selected_stack()
                    .unwrap()
                    .stack(AllocationStackFilter::Application)
                    .to_vec(),
                allocated_object
                    .related_stack()
                    .unwrap()
                    .stack(AllocationStackFilter::Application)
                    .to_vec(),
                (contention.thread_id, contention.objects[0].object_id),
                (arc.thread_id, arc.objects[0].object_id),
            ),
            (
                vec![1, 2, 3],
                (2, 100, 1, 1),
                vec!["app::allocate (producer.rs:10)".to_owned()],
                vec!["app::free (consumer.rs:20)".to_owned()],
                (2, 200),
                (3, 300),
            )
        );
    }
}
