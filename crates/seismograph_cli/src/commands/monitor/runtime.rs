// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime rows aggregate retained events, never the source's lifetime counters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use seismograph::recorder::event::{EventKind, Events};
use seismograph_runtime::model::{RuntimeModel, TaskKind, TaskState};
use seismograph_runtime::snapshot::{Snapshot, WorkerState};

use super::data::{AllocationStackFilter, primitive_stack};
use super::runtime_timeline::{ExecutionMetrics, Interval, TimeWindow, occupancy, union};

/// A bounded, common axis keeps old source metadata from stretching a chart over
/// the process lifetime. Ages retain their full value independently of this cap.
const MAX_WINDOW_NANOS: u64 = 60_000_000_000;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RuntimeMonitorSnapshot {
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) runtime_events: u64,
    pub(super) source_present: bool,
    pub(super) workers: Vec<RuntimeWorkerSummary>,
    pub(super) asynchronous_groups: Vec<AsynchronousTaskGroupSummary>,
    pub(super) blocking_groups: Vec<BlockingTaskGroupSummary>,
    pub(super) unclassified_tasks: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TaskOutcomeSummary {
    pub(super) tasks: u64,
    pub(super) active: u64,
    pub(super) completed: u64,
    pub(super) canceled: u64,
    pub(super) panicked: u64,
    pub(super) workers: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct DurationSummary {
    pub(super) samples: u64,
    pub(super) total_nanos: u64,
    pub(super) max_nanos: Option<u64>,
}

impl DurationSummary {
    pub(super) fn mean_nanos(&self) -> Option<u64> {
        (self.samples != 0).then(|| self.total_nanos / self.samples)
    }

    fn observe(&mut self, value: Option<u64>) {
        let Some(value) = value else { return };
        self.samples = self.samples.saturating_add(1);
        self.total_nanos = self.total_nanos.saturating_add(value);
        self.max_nanos = Some(self.max_nanos.map_or(value, |maximum| maximum.max(value)));
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct AsynchronousTaskGroupSummary {
    pub(super) type_descriptor_id: Option<u64>,
    pub(super) outcomes: TaskOutcomeSummary,
    pub(super) polls: u64,
    pub(super) poll_time: DurationSummary,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct BlockingTaskGroupSummary {
    pub(super) type_descriptor_id: Option<u64>,
    pub(super) outcomes: TaskOutcomeSummary,
    pub(super) executions: u64,
    pub(super) queue_time: DurationSummary,
    pub(super) execution_time: DurationSummary,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RuntimeWorkerSummary {
    pub(super) runtime_id: u64,
    pub(super) runtime_name: String,
    pub(super) worker_id: Option<u64>,
    pub(super) role: String,
    pub(super) state: String,
    pub(super) thread_id: Option<u64>,
    pub(super) current_task: Option<u64>,
    pub(super) observed_tasks: usize,
    pub(super) metrics: ExecutionMetrics,
    pub(super) window: Option<TimeWindow>,
    pub(super) tasks: Vec<RuntimeTaskSummary>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RuntimeTaskSummary {
    pub(super) task_id: u64,
    pub(super) runtime_id: u64,
    pub(super) parent_id: Option<u64>,
    pub(super) type_descriptor_id: Option<u64>,
    pub(super) future_size_bytes: Option<u64>,
    pub(super) state: String,
    pub(super) worker_ids: Vec<u64>,
    pub(super) spawn_stack: Vec<String>,
    pub(super) metrics: ExecutionMetrics,
    pub(super) activity: TaskActivitySummary,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TaskActivitySummary {
    pub(super) state: String,
    pub(super) running_for: Option<u64>,
    pub(super) ready_for: Option<u64>,
    pub(super) poll_worker_id: Option<u64>,
    pub(super) repoll_requested: bool,
}

impl Default for TaskActivitySummary {
    fn default() -> Self {
        Self {
            state: "Unknown".into(),
            running_for: None,
            ready_for: None,
            poll_worker_id: None,
            repoll_requested: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeTaskSort {
    Task,
    FutureSize,
    Polls,
    Executing,
    MedianPoll,
    MaximumPoll,
}

impl RuntimeTaskSort {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Task => Self::FutureSize,
            Self::FutureSize => Self::Polls,
            Self::Polls => Self::Executing,
            Self::Executing => Self::MedianPoll,
            Self::MedianPoll => Self::MaximumPoll,
            Self::MaximumPoll => Self::Task,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Task => Self::MaximumPoll,
            Self::FutureSize => Self::Task,
            Self::Polls => Self::FutureSize,
            Self::Executing => Self::Polls,
            Self::MedianPoll => Self::Executing,
            Self::MaximumPoll => Self::MedianPoll,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::FutureSize => "future bytes",
            Self::Polls => "polls",
            Self::Executing => "observed execution",
            Self::MedianPoll => "median poll",
            Self::MaximumPoll => "maximum poll",
        }
    }
}

impl RuntimeWorkerSummary {
    pub(super) fn sorted_tasks(&self, sort: RuntimeTaskSort, descending: bool) -> Vec<&RuntimeTaskSummary> {
        let mut tasks = self.tasks.iter().collect::<Vec<_>>();
        tasks.sort_unstable_by(|left, right| {
            let missing = |task: &RuntimeTaskSummary| match sort {
                RuntimeTaskSort::FutureSize => task.future_size_bytes.is_none(),
                RuntimeTaskSort::Executing => task.metrics.executing_fraction.is_none(),
                RuntimeTaskSort::MedianPoll => task.metrics.median_poll_nanos.is_none(),
                RuntimeTaskSort::MaximumPoll => task.metrics.max_poll_nanos.is_none(),
                _ => false,
            };
            let availability = missing(left).cmp(&missing(right));
            if !availability.is_eq() {
                return availability;
            }
            let ordering = match sort {
                RuntimeTaskSort::Task => left.task_id.cmp(&right.task_id),
                RuntimeTaskSort::FutureSize => left.future_size_bytes.cmp(&right.future_size_bytes),
                RuntimeTaskSort::Polls => left.metrics.poll_count.cmp(&right.metrics.poll_count),
                RuntimeTaskSort::Executing => left
                    .metrics
                    .executing_fraction
                    .partial_cmp(&right.metrics.executing_fraction)
                    .unwrap_or(std::cmp::Ordering::Equal),
                RuntimeTaskSort::MedianPoll => left.metrics.median_poll_nanos.cmp(&right.metrics.median_poll_nanos),
                RuntimeTaskSort::MaximumPoll => left.metrics.max_poll_nanos.cmp(&right.metrics.max_poll_nanos),
            };
            let ordering = if descending { ordering.reverse() } else { ordering };
            ordering.then_with(|| left.task_id.cmp(&right.task_id))
        });
        tasks
    }
}

#[derive(Default)]
struct TaskBuilder {
    row: RuntimeTaskSummary,
    workers: BTreeSet<u64>,
    executed: BTreeSet<Option<u64>>,
    metrics: BTreeMap<Option<u64>, ExecutionMetrics>,
    ready: Vec<Interval>,
    ready_samples: Vec<u64>,
    wake_samples: Vec<u64>,
    unassigned_poll: Option<Interval>,
    matching_poll_workers: BTreeSet<u64>,
}

#[derive(Default)]
struct TaskOutcomeBuilder {
    summary: TaskOutcomeSummary,
    workers: BTreeSet<u64>,
}

impl TaskOutcomeBuilder {
    fn observe(&mut self, task: &seismograph_runtime::model::Task) {
        self.summary.tasks = self.summary.tasks.saturating_add(1);
        match task.state {
            TaskState::Queued | TaskState::Running | TaskState::Waiting => {
                self.summary.active = self.summary.active.saturating_add(1);
            }
            TaskState::Completed => self.summary.completed = self.summary.completed.saturating_add(1),
            TaskState::Cancelled => self.summary.canceled = self.summary.canceled.saturating_add(1),
            TaskState::Panicked => self.summary.panicked = self.summary.panicked.saturating_add(1),
            _ => {}
        }
        self.workers.extend(task.workers.iter().map(|worker| worker.get()));
    }

    fn finish(mut self) -> TaskOutcomeSummary {
        self.summary.workers = u64::try_from(self.workers.len()).unwrap_or(u64::MAX);
        self.summary
    }
}

#[derive(Default)]
struct AsynchronousTaskGroupBuilder {
    outcomes: TaskOutcomeBuilder,
    polls: u64,
    poll_time: DurationSummary,
}

#[derive(Default)]
struct BlockingTaskGroupBuilder {
    outcomes: TaskOutcomeBuilder,
    executions: u64,
    queue_time: DurationSummary,
    execution_time: DurationSummary,
}

fn observe_task_group(
    task: &seismograph_runtime::model::Task,
    descriptor: Option<u64>,
    asynchronous_groups: &mut BTreeMap<Option<u64>, AsynchronousTaskGroupBuilder>,
    blocking_groups: &mut BTreeMap<Option<u64>, BlockingTaskGroupBuilder>,
    unclassified_tasks: &mut u64,
) {
    match &task.kind {
        TaskKind::Asynchronous(asynchronous) => {
            let group = asynchronous_groups.entry(descriptor).or_default();
            group.outcomes.observe(task);
            group.polls = group
                .polls
                .saturating_add(u64::try_from(asynchronous.polls.len()).unwrap_or(u64::MAX));
            for poll in &asynchronous.polls {
                group.poll_time.observe(poll.duration_nanos);
            }
        }
        TaskKind::Blocking(blocking) => {
            let group = blocking_groups.entry(descriptor).or_default();
            group.outcomes.observe(task);
            group.executions = group.executions.saturating_add(u64::from(blocking.started_at.is_some()));
            group.queue_time.observe(blocking.queue_duration_nanos);
            group.execution_time.observe(blocking.duration_nanos);
        }
        _ => {
            *unclassified_tasks = unclassified_tasks.saturating_add(1);
        }
    }
}

fn finish_task_groups(
    asynchronous_groups: BTreeMap<Option<u64>, AsynchronousTaskGroupBuilder>,
    blocking_groups: BTreeMap<Option<u64>, BlockingTaskGroupBuilder>,
) -> (Vec<AsynchronousTaskGroupSummary>, Vec<BlockingTaskGroupSummary>) {
    let mut asynchronous = asynchronous_groups
        .into_iter()
        .map(|(type_descriptor_id, group)| AsynchronousTaskGroupSummary {
            type_descriptor_id,
            outcomes: group.outcomes.finish(),
            polls: group.polls,
            poll_time: group.poll_time,
        })
        .collect::<Vec<_>>();
    asynchronous.sort_unstable_by_key(|group| {
        (
            std::cmp::Reverse(group.polls),
            std::cmp::Reverse(group.outcomes.tasks),
            group.type_descriptor_id.unwrap_or(u64::MAX),
        )
    });
    let mut blocking = blocking_groups
        .into_iter()
        .map(|(type_descriptor_id, group)| BlockingTaskGroupSummary {
            type_descriptor_id,
            outcomes: group.outcomes.finish(),
            executions: group.executions,
            queue_time: group.queue_time,
            execution_time: group.execution_time,
        })
        .collect::<Vec<_>>();
    blocking.sort_unstable_by_key(|group| {
        (
            std::cmp::Reverse(group.executions),
            std::cmp::Reverse(group.outcomes.tasks),
            group.type_descriptor_id.unwrap_or(u64::MAX),
        )
    });
    (asynchronous, blocking)
}

impl TaskBuilder {
    fn poll_started(&mut self, worker_id: Option<u64>, timestamp: u64, duration: u64, flag: u64) {
        self.executed.insert(worker_id);
        if self.unassigned_poll.is_some_and(|poll| poll.start == timestamp)
            && let Some(worker_id) = worker_id
        {
            self.matching_poll_workers.insert(worker_id);
        }
        if timestamp == 0 {
            return;
        }
        let Some(start) = timestamp.checked_sub(duration) else { return };
        match flag {
            2 => {
                self.ready.push(Interval { start, end: timestamp });
                self.ready_samples.push(duration);
            }
            // Raw wake latency can include the preceding running poll.
            // Keep it useful, but never draw it as pure queue waiting.
            1 => self.wake_samples.push(duration),
            _ => {}
        }
    }
}

type Workers = BTreeMap<(u64, Option<u64>), RuntimeWorkerSummary>;

#[derive(Default)]
struct RuntimeSeed {
    workers: Workers,
    tasks: BTreeMap<(u64, u64), TaskBuilder>,
    asynchronous_groups: BTreeMap<Option<u64>, AsynchronousTaskGroupBuilder>,
    blocking_groups: BTreeMap<Option<u64>, BlockingTaskGroupBuilder>,
    unclassified_tasks: u64,
}

fn seed_runtime_model(events: &Events, source: Option<&Snapshot>) -> RuntimeSeed {
    let mut seed = RuntimeSeed::default();
    for runtime in RuntimeModel::from_events(events, source).runtimes {
        for worker in runtime.workers {
            seed.workers.insert(
                (runtime.id.get(), Some(worker.id.get())),
                RuntimeWorkerSummary {
                    runtime_id: runtime.id.get(),
                    runtime_name: runtime.name.clone(),
                    worker_id: Some(worker.id.get()),
                    role: worker.role.map_or_else(|| "Unknown".into(), |role| format!("{role:?}")),
                    state: worker_state(worker.state),
                    thread_id: worker.thread_id.map(seismograph::recorder::thread::ThreadId::get),
                    current_task: worker.current_task.map(seismograph::recorder::runtime::TaskId::get),
                    ..RuntimeWorkerSummary::default()
                },
            );
        }
        for task in runtime.tasks {
            let descriptor = task.type_descriptor.map(seismograph::recorder::runtime::TypeDescriptorId::get);
            observe_task_group(
                &task,
                descriptor,
                &mut seed.asynchronous_groups,
                &mut seed.blocking_groups,
                &mut seed.unclassified_tasks,
            );
            let key = (task.runtime_id.get(), task.id.get());
            let mut builder = TaskBuilder {
                row: RuntimeTaskSummary {
                    task_id: task.id.get(),
                    runtime_id: task.runtime_id.get(),
                    parent_id: task.parent.map(seismograph::recorder::runtime::TaskId::get),
                    type_descriptor_id: task.type_descriptor.map(seismograph::recorder::runtime::TypeDescriptorId::get),
                    future_size_bytes: task.body_size,
                    state: task_state(task.state).into(),
                    worker_ids: task.workers.iter().map(|worker| worker.get()).collect(),
                    ..RuntimeTaskSummary::default()
                },
                ..TaskBuilder::default()
            };
            builder.row.activity.state.clone_from(&builder.row.state);
            builder
                .workers
                .extend(task.workers.into_iter().map(seismograph::recorder::runtime::WorkerId::get));
            seed.tasks.insert(key, builder);
        }
    }
    seed
}

const fn task_state(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "Ready",
        TaskState::Running => "Running",
        TaskState::Waiting => "Waiting",
        TaskState::Completed => "Completed",
        TaskState::Cancelled => "Canceled",
        TaskState::Panicked => "Panicked",
        _ => "Unknown",
    }
}

impl RuntimeMonitorSnapshot {
    pub(super) fn from_events(
        events: &Events,
        source: Option<&Snapshot>,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
    ) -> Self {
        let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
        let RuntimeSeed {
            mut workers,
            mut tasks,
            asynchronous_groups,
            blocking_groups,
            unclassified_tasks,
        } = seed_runtime_model(events, source);
        for event in &events.events {
            let Some(runtime) = event.runtime() else { continue };
            let runtime_id = runtime.runtime_id.get();
            let worker_id = runtime.worker_id.map(seismograph::recorder::runtime::WorkerId::get);
            // Queue origins and ready notifiers are not execution workers.
            let executor_worker_id =
                worker_id.filter(|_| !matches!(event.kind, EventKind::TaskSpawned | EventKind::TaskEnqueued | EventKind::TaskReady));
            if let Some(worker_id) = executor_worker_id {
                workers
                    .entry((runtime_id, Some(worker_id)))
                    .or_insert_with(|| worker_row(runtime_id, Some(worker_id)));
            }

            let Some(task_id) = runtime_task_id(event.kind, runtime.subject_id, runtime.related_id) else {
                continue;
            };
            let task = tasks.entry((runtime_id, task_id)).or_default();
            task.row.task_id = task_id;
            task.row.runtime_id = runtime_id;
            if task.row.state.is_empty() {
                task.row.state = "Unknown".into();
            }
            if let Some(worker_id) = executor_worker_id {
                task.workers.insert(worker_id);
            }
            let timestamp = event.timestamp.ticks();
            match event.kind {
                EventKind::TaskSpawned | EventKind::TaskEnqueued => {
                    if task.row.spawn_stack.is_empty() {
                        let stack = event.call_stack.iter().map(|address| address.get()).collect::<Vec<_>>();
                        task.row.spawn_stack = primitive_stack(&stack, &lookups, AllocationStackFilter::All);
                    }
                }
                EventKind::TaskPollStarted => {
                    task.poll_started(worker_id, timestamp, runtime.value_0, runtime.value_1);
                }
                EventKind::TaskPollFinished => {
                    task.executed.insert(worker_id);
                    let metrics = task.metrics.entry(worker_id).or_default();
                    metrics.poll_samples.push(runtime.value_0);
                    if timestamp != 0
                        && let Some(start) = timestamp.checked_sub(runtime.value_0)
                    {
                        metrics.polls.push(Interval { start, end: timestamp });
                    }
                }
                _ => {}
            }
        }
        distribute_tasks(&mut workers, tasks);
        let windows = display_windows(events, source);
        let (asynchronous_groups, blocking_groups) = finish_task_groups(asynchronous_groups, blocking_groups);
        let mut snapshot = Self {
            total_events: events.total_events,
            retained_events: u64::try_from(events.events.len()).unwrap_or(u64::MAX),
            lost_events: events.lost_events,
            runtime_events: u64::try_from(events.events.iter().filter(|event| event.runtime().is_some()).count()).unwrap_or(u64::MAX),
            source_present: source.is_some(),
            workers: workers.into_values().collect(),
            asynchronous_groups,
            blocking_groups,
            unclassified_tasks,
        };
        for worker in &mut snapshot.workers {
            if let Some(runtime) = source.and_then(|source| source.runtimes.iter().find(|runtime| runtime.id.get() == worker.runtime_id)) {
                worker.runtime_name.clone_from(&runtime.name);
            }
            worker.metrics.finish(windows.get(&worker.runtime_id).copied());
            for task in &mut worker.tasks {
                task.metrics.finish(windows.get(&worker.runtime_id).copied());
            }
        }
        snapshot.set_windows(&windows);
        snapshot
    }

    /// Filtering changes evidence, not the denominator of the displayed capture.
    pub(super) fn set_windows(&mut self, windows: &BTreeMap<u64, TimeWindow>) {
        for worker in &mut self.workers {
            worker.window = windows.get(&worker.runtime_id).copied();
            worker.metrics.executing_fraction = worker.window.and_then(|window| occupancy(&worker.metrics.polls, window));
            for task in &mut worker.tasks {
                task.metrics.executing_fraction = worker.window.and_then(|window| occupancy(&task.metrics.polls, window));
            }
        }
    }
}

fn distribute_tasks(workers: &mut Workers, tasks: BTreeMap<(u64, u64), TaskBuilder>) {
    for ((runtime_id, _), mut task) in tasks {
        if let Some(poll) = task.unassigned_poll
            && task.matching_poll_workers.len() == 1
            && let Some(worker) = task.matching_poll_workers.first().copied()
        {
            task.metrics.entry(Some(worker)).or_default().polls.push(poll);
            task.row.activity.poll_worker_id = Some(worker);
        }
        task.row.worker_ids = task.workers.iter().copied().collect();
        let mut assignments = task.workers.iter().copied().map(Some).collect::<BTreeSet<_>>();
        if assignments.is_empty() || task.metrics.contains_key(&None) || task.executed.contains(&None) {
            assignments.insert(None);
        }
        let ready: Arc<[Interval]> = union(&task.ready).into();
        let ready_samples: Arc<[u64]> = task.ready_samples.into();
        let wake_samples: Arc<[u64]> = task.wake_samples.into();
        for worker_id in assignments {
            let worker = workers
                .entry((runtime_id, worker_id))
                .or_insert_with(|| worker_row(runtime_id, worker_id));
            worker.observed_tasks += usize::from(task.executed.contains(&worker_id));
            let mut row = task.row.clone();
            row.metrics = task.metrics.remove(&worker_id).unwrap_or_default();
            row.metrics.ready = Arc::clone(&ready);
            row.metrics.ready_samples = Arc::clone(&ready_samples);
            row.metrics.wake_samples = Arc::clone(&wake_samples);
            worker.metrics.polls.extend_from_slice(&row.metrics.polls);
            worker.metrics.poll_samples.extend_from_slice(&row.metrics.poll_samples);
            worker.tasks.push(row);
        }
    }
}

fn worker_row(runtime_id: u64, worker_id: Option<u64>) -> RuntimeWorkerSummary {
    RuntimeWorkerSummary {
        runtime_id,
        runtime_name: format!("runtime #{runtime_id}"),
        worker_id,
        role: if worker_id.is_none() { "Unbound" } else { "Unknown" }.into(),
        state: "Unknown".into(),
        ..RuntimeWorkerSummary::default()
    }
}

fn worker_state(state: WorkerState) -> String {
    match state {
        WorkerState::Running => "Running",
        WorkerState::Parked => "Parked",
        WorkerState::Stopped => "Stopped",
        _ => "Unknown",
    }
    .into()
}

pub(super) fn display_windows(events: &Events, _source: Option<&Snapshot>) -> BTreeMap<u64, TimeWindow> {
    let mut bounds = BTreeMap::<u64, TimeWindow>::new();
    let mut observe = |runtime: u64, start: u64, end: u64| {
        if end == 0 {
            return;
        }
        if end.checked_sub(start).is_none() {
            return;
        }
        let window = bounds.entry(runtime).or_insert(TimeWindow { start, end });
        window.start = window.start.min(start);
        window.end = window.end.max(end);
    };
    for event in &events.events {
        if let Some(runtime) = event.runtime() {
            let end = event.timestamp.ticks();
            let start = if event.kind == EventKind::TaskPollFinished {
                end.checked_sub(runtime.value_0).unwrap_or(end)
            } else {
                end
            };
            observe(runtime.runtime_id.get(), start, end);
        }
    }
    bounds.retain(|_, window| {
        window.start = window.start.max(window.end.saturating_sub(MAX_WINDOW_NANOS));
        window.end > window.start
    });
    bounds
}

pub(super) fn runtime_task_id(kind: EventKind, subject_id: u64, related_id: u64) -> Option<u64> {
    match kind {
        EventKind::TaskSpawned
        | EventKind::TaskEnqueued
        | EventKind::TaskMaterialized
        | EventKind::TaskReady
        | EventKind::TaskPollStarted
        | EventKind::TaskPollFinished
        | EventKind::TaskCompleted
        | EventKind::TaskCanceled
        | EventKind::TaskPanicked => (subject_id != 0).then_some(subject_id),
        EventKind::TransferStarted | EventKind::InstanceRelocated | EventKind::TransferFinished => (related_id != 0).then_some(related_id),
        _ => None,
    }
}
