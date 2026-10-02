// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime rows aggregate retained events, never the source's lifetime counters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use seismograph::recorder::event::{EventKind, Events};
use seismograph_runtime::snapshot::{Snapshot, TaskActivity, TaskActivityState};

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

impl RuntimeMonitorSnapshot {
    pub(super) fn from_events(
        events: &Events,
        source: Option<&Snapshot>,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
    ) -> Self {
        let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
        let mut workers = Workers::new();
        let mut tasks = BTreeMap::<(u64, u64), TaskBuilder>::new();
        import_source(source, &lookups, &mut workers, &mut tasks);
        for event in &events.events {
            let Some(runtime) = event.runtime() else { continue };
            let runtime_id = runtime.runtime_id.get();
            let worker_id = runtime.worker_id.map(seismograph::recorder::runtime::WorkerId::get);
            // TaskReady is emitted on the notifier's OS thread, not an executor worker.
            let executor_worker_id = worker_id.filter(|_| event.kind != EventKind::TaskReady);
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
                EventKind::TaskSpawned => {
                    task.row.parent_id = (runtime.related_id != 0).then_some(runtime.related_id);
                    task.row.type_descriptor_id = (runtime.value_0 != 0).then_some(runtime.value_0);
                    if let Some(size) = runtime.value_1.checked_sub(1) {
                        task.row.future_size_bytes = Some(size);
                    }
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
                EventKind::TaskCompleted | EventKind::TaskCanceled | EventKind::TaskPanicked => {
                    task.row.state = match event.kind {
                        EventKind::TaskCompleted => "Completed",
                        EventKind::TaskCanceled => "Canceled",
                        _ => "Panicked",
                    }
                    .into();
                    task.row.activity = TaskActivitySummary {
                        state: task.row.state.clone(),
                        ..TaskActivitySummary::default()
                    };
                }
                _ => {}
            }
        }
        distribute_tasks(&mut workers, tasks);
        let windows = display_windows(events, source);
        let mut snapshot = Self {
            total_events: events.total_events,
            retained_events: u64::try_from(events.events.len()).unwrap_or(u64::MAX),
            lost_events: events.lost_events,
            runtime_events: u64::try_from(events.events.iter().filter(|event| event.runtime().is_some()).count()).unwrap_or(u64::MAX),
            source_present: source.is_some(),
            workers: workers.into_values().collect(),
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

fn import_source(
    source: Option<&Snapshot>,
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    workers: &mut Workers,
    tasks: &mut BTreeMap<(u64, u64), TaskBuilder>,
) {
    let Some(source) = source else { return };
    for runtime in &source.runtimes {
        let runtime_id = runtime.id.get();
        for worker in &runtime.workers {
            workers.insert(
                (runtime_id, Some(worker.id.get())),
                RuntimeWorkerSummary {
                    runtime_id,
                    worker_id: Some(worker.id.get()),
                    role: format!("{:?}", worker.role),
                    state: format!("{:?}", worker.state),
                    thread_id: worker.thread_id.map(seismograph::recorder::thread::ThreadId::get),
                    current_task: worker.current_task.map(seismograph::recorder::runtime::TaskId::get),
                    ..RuntimeWorkerSummary::default()
                },
            );
        }
        for source_task in &runtime.tasks {
            let task = tasks.entry((runtime_id, source_task.id.get())).or_default();
            let stack = source_task.spawn_backtrace.iter().map(|address| address.get()).collect::<Vec<_>>();
            task.row = RuntimeTaskSummary {
                runtime_id,
                task_id: source_task.id.get(),
                parent_id: source_task.parent.map(seismograph::recorder::runtime::TaskId::get),
                type_descriptor_id: Some(source_task.type_descriptor.get()),
                future_size_bytes: source_task.future_size_bytes,
                spawn_stack: primitive_stack(&stack, lookups, AllocationStackFilter::All),
                ..RuntimeTaskSummary::default()
            };
            if let Some(worker) = source_task.last_worker_id {
                task.workers.insert(worker.get());
            }
            apply_activity(task, source_task.activity);
        }
        // A source can publish worker association independently of task metadata.
        for worker in &runtime.workers {
            if let Some(task_id) = worker.current_task {
                let task = tasks.entry((runtime_id, task_id.get())).or_default();
                task.row.runtime_id = runtime_id;
                task.row.task_id = task_id.get();
                task.workers.insert(worker.id.get());
                if task.row.state.is_empty() {
                    task.row.state = "Unknown".into();
                }
            }
        }
    }
}

fn apply_activity(task: &mut TaskBuilder, activity: Option<TaskActivity>) {
    task.row.state = "Unknown".into();
    let Some(activity) = activity else { return };
    let observed = activity.observed_at.ticks();
    if observed == 0 {
        return;
    }
    match activity.state {
        TaskActivityState::Running => {
            let Some(started_at) = activity.poll_started_at.map(seismograph::recorder::event::EventTimestamp::ticks) else {
                return;
            };
            if activity.queued_since.is_some() {
                return;
            }
            if started_at > observed {
                return;
            }
            if let Some(ready_at) = activity.ready_since.map(seismograph::recorder::event::EventTimestamp::ticks) {
                if ready_at < started_at {
                    return;
                }
                if ready_at > observed {
                    return;
                }
            }
            let running_for = observed - started_at;
            task.row.activity = TaskActivitySummary {
                state: "Running".into(),
                running_for: Some(running_for),
                ready_for: None,
                poll_worker_id: activity.poll_worker_id.map(seismograph::recorder::runtime::WorkerId::get),
                repoll_requested: activity.ready_since.is_some(),
            };
            // Only the worker identity inside the coherent activity is evidence
            // of this poll's placement; last_worker/current_task can race.
            if let Some(worker_id) = task.row.activity.poll_worker_id {
                task.workers.insert(worker_id);
                task.executed.insert(Some(worker_id));
                task.metrics.entry(Some(worker_id)).or_default().polls.push(Interval {
                    start: started_at,
                    end: observed,
                });
            } else {
                task.unassigned_poll = Some(Interval {
                    start: started_at,
                    end: observed,
                });
            }
        }
        TaskActivityState::Ready => {
            let (Some(ready_at), Some(queued_at)) = (
                activity.ready_since.map(seismograph::recorder::event::EventTimestamp::ticks),
                activity.queued_since.map(seismograph::recorder::event::EventTimestamp::ticks),
            ) else {
                return;
            };
            if activity.poll_started_at.is_some() {
                return;
            }
            if activity.poll_worker_id.is_some() {
                return;
            }
            if ready_at > queued_at {
                return;
            }
            if queued_at > observed {
                return;
            }
            let ready_for = observed - queued_at;
            task.row.activity = TaskActivitySummary {
                state: "Ready".into(),
                ready_for: Some(ready_for),
                ..TaskActivitySummary::default()
            };
            task.ready.push(Interval {
                start: queued_at,
                end: observed,
            });
        }
        TaskActivityState::Waiting => {
            if matches!(
                (
                    activity.poll_started_at,
                    activity.ready_since,
                    activity.queued_since,
                    activity.poll_worker_id,
                ),
                (None, None, None, None)
            ) {
                task.row.activity.state = "Waiting".into();
            }
        }
        _ => {}
    }
    task.row.state.clone_from(&task.row.activity.state);
}

pub(super) fn display_windows(events: &Events, source: Option<&Snapshot>) -> BTreeMap<u64, TimeWindow> {
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
    if let Some(source) = source {
        for runtime in &source.runtimes {
            for task in &runtime.tasks {
                if let Some(activity) = task.activity {
                    let end = activity.observed_at.ticks();
                    let start = match activity.state {
                        TaskActivityState::Running => activity.poll_started_at,
                        TaskActivityState::Ready => activity.queued_since,
                        _ => None,
                    }
                    .map_or(end, seismograph::recorder::event::EventTimestamp::ticks);
                    observe(runtime.id.get(), start, end);
                }
            }
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

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{Address, BacktraceCapture, Event, EventPayload, EventSequence, EventTimestamp};
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TypeDescriptorId, WorkerId};
    use seismograph::recorder::thread::ThreadId;
    use seismograph_runtime::snapshot::{Counters, Runtime, RuntimeState, Task, TaskMetrics, Worker, WorkerState};
    use seismograph_runtime::worker::WorkerRole;

    use super::*;

    fn event(kind: EventKind, worker: Option<u64>, timestamp: u64, value: u64, flag: u64) -> Event {
        Event {
            thread_id: ThreadId::new(99),
            sequence: EventSequence::new(timestamp),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: worker.and_then(WorkerId::from_raw),
                subject_id: 1,
                related_id: 0,
                value_0: value,
                value_1: flag,
            }),
            call_stack: Vec::new(),
        }
    }

    fn source(activity: Option<TaskActivity>) -> Snapshot {
        Snapshot {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "executor".into(),
                configured_workers: 1,
                lifecycle_backtraces: BacktraceCapture::Never,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(1),
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
                tasks: vec![Task {
                    id: TaskId::from_raw(1).unwrap(),
                    parent: None,
                    type_descriptor: TypeDescriptorId::from_raw(1).unwrap(),
                    future_size_bytes: None,
                    spawned_at: EventTimestamp::from_ticks(1),
                    last_worker_id: WorkerId::from_raw(1),
                    metrics: TaskMetrics {
                        poll_count: 100_000,
                        ..TaskMetrics::default()
                    },
                    activity,
                    spawn_backtrace: Vec::new(),
                }],
            }],
            addresses: Vec::new(),
        }
    }

    #[test]
    fn completed_task_sizes_survive_without_live_source_metadata() {
        let sizes = [0, 1, 16_385].map(|encoded| {
            let events = Events {
                events: vec![
                    event(EventKind::TaskSpawned, None, 10, 1, encoded),
                    event(EventKind::TaskCompleted, Some(1), 20, 0, 0),
                ],
                ..Events::default()
            };
            RuntimeMonitorSnapshot::from_events(&events, None, &[]).workers[0].tasks[0].future_size_bytes
        });
        assert_eq!(sizes, [None, Some(0), Some(16_384)]);
    }

    #[test]
    fn spawned_task_preserves_nonzero_parent_identity() {
        let mut spawned = event(EventKind::TaskSpawned, None, 10, 1, 1);
        if let EventPayload::Runtime(runtime) = &mut spawned.payload {
            runtime.related_id = 7;
        }
        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                events: vec![spawned],
                ..Events::default()
            },
            None,
            &[],
        );
        assert_eq!(snapshot.workers[0].tasks[0].parent_id, Some(7));
    }

    #[test]
    fn repeated_spawn_events_preserve_the_first_backtrace() {
        let mut first = event(EventKind::TaskSpawned, None, 10, 1, 1);
        first.call_stack = vec![Address::new(0x1000)];
        let mut repeated = event(EventKind::TaskSpawned, None, 11, 1, 1);
        repeated.call_stack = vec![Address::new(0x2000)];

        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                events: vec![first, repeated],
                ..Events::default()
            },
            None,
            &[],
        );

        assert!(snapshot.workers[0].tasks[0].spawn_stack.iter().any(|frame| frame.contains("1000")));
    }

    #[test]
    fn ready_activity_requires_both_notification_and_queue_timestamps() {
        for (ready_since, queued_since) in [(None, Some(90)), (Some(90), None)] {
            let mut task = TaskBuilder::default();
            apply_activity(
                &mut task,
                Some(TaskActivity {
                    observed_at: EventTimestamp::from_ticks(100),
                    state: TaskActivityState::Ready,
                    ready_since: ready_since.map(EventTimestamp::from_ticks),
                    poll_started_at: None,
                    poll_worker_id: None,
                    queued_since: queued_since.map(EventTimestamp::from_ticks),
                }),
            );

            assert_eq!((task.row.activity, task.ready), (TaskActivitySummary::default(), Vec::new()));
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one state matrix verifies every coherent and rejected activity shape"
    )]
    fn activity_validation_rejects_incoherent_states_and_accepts_waiting() {
        let mut task = TaskBuilder::default();
        apply_activity(&mut task, None);
        assert_eq!(task.row.activity, TaskActivitySummary::default());

        for activity in [
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(0),
                state: TaskActivityState::Unknown,
                ready_since: None,
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: None,
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Ready,
                ready_since: Some(EventTimestamp::from_ticks(101)),
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: Some(EventTimestamp::from_ticks(101)),
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Running,
                ready_since: None,
                poll_started_at: None,
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: None,
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Ready,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: Some(EventTimestamp::from_ticks(80)),
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Running,
                ready_since: Some(EventTimestamp::from_ticks(80)),
                poll_started_at: Some(EventTimestamp::from_ticks(90)),
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: None,
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Running,
                ready_since: None,
                poll_started_at: Some(EventTimestamp::from_ticks(90)),
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: Some(EventTimestamp::from_ticks(90)),
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Running,
                ready_since: None,
                poll_started_at: Some(EventTimestamp::from_ticks(101)),
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: None,
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Running,
                ready_since: Some(EventTimestamp::from_ticks(101)),
                poll_started_at: Some(EventTimestamp::from_ticks(90)),
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: None,
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Ready,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: Some(EventTimestamp::from_ticks(90)),
                poll_worker_id: None,
                queued_since: Some(EventTimestamp::from_ticks(90)),
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Ready,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: None,
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: Some(EventTimestamp::from_ticks(90)),
            },
            TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Waiting,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: None,
            },
        ] {
            apply_activity(&mut task, Some(activity));
            assert_eq!(task.row.activity, TaskActivitySummary::default());
        }

        let mut running = TaskBuilder::default();
        apply_activity(
            &mut running,
            Some(TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Running,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: Some(EventTimestamp::from_ticks(90)),
                poll_worker_id: WorkerId::from_raw(1),
                queued_since: None,
            }),
        );
        assert_eq!(
            (running.row.activity.state.as_str(), running.row.activity.running_for),
            ("Running", Some(10))
        );

        let mut ready = TaskBuilder::default();
        apply_activity(
            &mut ready,
            Some(TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Ready,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: Some(EventTimestamp::from_ticks(90)),
            }),
        );
        assert_eq!(
            (ready.row.activity.state.as_str(), ready.row.activity.ready_for),
            ("Ready", Some(10))
        );

        for (activity, expected) in [
            (
                TaskActivity {
                    observed_at: EventTimestamp::from_ticks(100),
                    state: TaskActivityState::Running,
                    ready_since: None,
                    poll_started_at: Some(EventTimestamp::from_ticks(100)),
                    poll_worker_id: WorkerId::from_raw(1),
                    queued_since: None,
                },
                ("Running", Some(0), false),
            ),
            (
                TaskActivity {
                    observed_at: EventTimestamp::from_ticks(100),
                    state: TaskActivityState::Running,
                    ready_since: Some(EventTimestamp::from_ticks(100)),
                    poll_started_at: Some(EventTimestamp::from_ticks(90)),
                    poll_worker_id: WorkerId::from_raw(1),
                    queued_since: None,
                },
                ("Running", Some(10), true),
            ),
        ] {
            let mut task = TaskBuilder::default();
            apply_activity(&mut task, Some(activity));
            assert_eq!(
                (
                    task.row.activity.state.as_str(),
                    task.row.activity.running_for,
                    task.row.activity.repoll_requested,
                ),
                expected
            );
        }
        let mut ready_at_observation = TaskBuilder::default();
        apply_activity(
            &mut ready_at_observation,
            Some(TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Ready,
                ready_since: Some(EventTimestamp::from_ticks(90)),
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: Some(EventTimestamp::from_ticks(100)),
            }),
        );
        assert_eq!(ready_at_observation.row.activity.ready_for, Some(0));

        apply_activity(
            &mut task,
            Some(TaskActivity {
                observed_at: EventTimestamp::from_ticks(100),
                state: TaskActivityState::Waiting,
                ready_since: None,
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: None,
            }),
        );
        assert_eq!(task.row.activity.state, "Waiting");
    }

    #[test]
    fn legacy_spawn_event_does_not_erase_known_source_future_size() {
        let mut source = source(None);
        source.runtimes[0].tasks[0].future_size_bytes = Some(88);
        let events = Events {
            events: vec![event(EventKind::TaskSpawned, Some(1), 10, 1, 0)],
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source), &[]);
        assert_eq!(snapshot.workers[0].tasks[0].future_size_bytes, Some(88));
    }

    #[test]
    fn future_size_sort_keeps_unknown_last_and_zero_distinct() {
        let worker = RuntimeWorkerSummary {
            tasks: [(1, Some(32)), (2, None), (3, Some(0))]
                .map(|(task_id, future_size_bytes)| RuntimeTaskSummary {
                    task_id,
                    future_size_bytes,
                    ..RuntimeTaskSummary::default()
                })
                .to_vec(),
            ..RuntimeWorkerSummary::default()
        };
        let ids = |descending| {
            worker
                .sorted_tasks(RuntimeTaskSort::FutureSize, descending)
                .iter()
                .map(|task| task.task_id)
                .collect::<Vec<_>>()
        };
        assert_eq!((ids(false), ids(true)), (vec![3, 1, 2], vec![1, 3, 2]));
    }

    #[test]
    fn executing_sort_uses_fraction_and_keeps_unknown_last() {
        let worker = RuntimeWorkerSummary {
            tasks: [(1, Some(0.75)), (2, None), (3, Some(0.25))]
                .map(|(task_id, executing_fraction)| RuntimeTaskSummary {
                    task_id,
                    metrics: ExecutionMetrics {
                        executing_fraction,
                        ..ExecutionMetrics::default()
                    },
                    ..RuntimeTaskSummary::default()
                })
                .to_vec(),
            ..RuntimeWorkerSummary::default()
        };
        let ids = |descending| {
            worker
                .sorted_tasks(RuntimeTaskSort::Executing, descending)
                .iter()
                .map(|task| task.task_id)
                .collect::<Vec<_>>()
        };
        assert_eq!((ids(false), ids(true)), (vec![3, 1, 2], vec![1, 3, 2]));
    }

    #[test]
    fn task_ready_worker_identity_is_not_executor_placement() {
        let events = Events {
            events: vec![event(EventKind::TaskReady, Some(99), 10, 0, 0)],
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, None, &[]);
        assert_eq!(
            snapshot
                .workers
                .iter()
                .map(|worker| (worker.worker_id, worker.tasks[0].worker_ids.clone()))
                .collect::<Vec<_>>(),
            [(None, Vec::new())]
        );
    }

    #[test]
    fn unbound_assignment_retains_metrics_or_execution_without_worker_identity() {
        let mut with_metrics = TaskBuilder::default();
        with_metrics.row.task_id = 1;
        with_metrics.row.runtime_id = 1;
        with_metrics.workers.insert(7);
        with_metrics.metrics.insert(None, ExecutionMetrics::default());
        let mut with_execution = TaskBuilder::default();
        with_execution.row.task_id = 2;
        with_execution.row.runtime_id = 1;
        with_execution.workers.insert(7);
        with_execution.executed.insert(None);
        let mut workers = Workers::new();
        distribute_tasks(&mut workers, BTreeMap::from([((1, 1), with_metrics), ((1, 2), with_execution)]));
        assert_eq!(
            workers
                .get(&(1, None))
                .unwrap()
                .tasks
                .iter()
                .map(|task| task.task_id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn source_import_preserves_runtime_and_task_identity_metadata() {
        let mut source = source(None);
        source.runtimes[0].name = "named-runtime".into();
        source.runtimes[0].workers[0].current_task = None;
        source.runtimes[0].tasks[0].parent = TaskId::from_raw(7);
        source.runtimes[0].tasks[0].type_descriptor = TypeDescriptorId::from_raw(9).unwrap();
        source.runtimes[0].tasks[0].last_worker_id = None;
        source.runtimes[0].tasks[0].spawn_backtrace = vec![Address::new(0x1234)];
        let snapshot = RuntimeMonitorSnapshot::from_events(&Events::default(), Some(&source), &[]);
        let worker = &snapshot.workers[0];
        let task = &worker.tasks[0];
        assert_eq!(
            (
                worker.runtime_id,
                worker.runtime_name.as_str(),
                task.runtime_id,
                task.task_id,
                task.parent_id,
                task.type_descriptor_id,
                task.spawn_stack.len(),
            ),
            (1, "named-runtime", 1, 1, Some(7), Some(9), 1)
        );
    }

    #[test]
    fn ten_minute_running_and_ready_are_distinct_and_not_completed_samples() {
        let start = 1_000;
        let age = 600_000_000_000;
        for state in [TaskActivityState::Running, TaskActivityState::Ready] {
            let running = state == TaskActivityState::Running;
            let source = source(Some(TaskActivity {
                observed_at: EventTimestamp::from_ticks(start + age),
                state,
                ready_since: (!running).then_some(EventTimestamp::from_ticks(start)),
                poll_started_at: running.then_some(EventTimestamp::from_ticks(start)),
                poll_worker_id: running.then(|| WorkerId::from_raw(1).unwrap()),
                queued_since: (!running).then_some(EventTimestamp::from_ticks(start)),
            }));
            let snapshot = RuntimeMonitorSnapshot::from_events(&Events::default(), Some(&source), &[]);
            let worker = &snapshot.workers[0];
            let task = &worker.tasks[0];
            assert_eq!(
                (task.activity.running_for, task.activity.ready_for),
                (running.then_some(age), (!running).then_some(age))
            );
            assert_eq!(
                worker.window,
                Some(TimeWindow {
                    start: start + age - MAX_WINDOW_NANOS,
                    end: start + age
                })
            );
            assert_eq!(worker.metrics.executing_fraction, running.then_some(1.0));
            assert_eq!((worker.metrics.poll_count, task.metrics.poll_count), (0, 0));
            assert_eq!((task.metrics.median_poll_nanos, task.metrics.max_poll_nanos), (None, None));
            assert!(task.metrics.poll_samples.is_empty() && task.metrics.ready_samples.is_empty());
        }
    }

    #[test]
    fn missing_finish_is_not_evidence_of_an_open_poll_or_ready_wait() {
        let events = Events {
            events: vec![
                event(EventKind::TaskPollStarted, Some(1), 10, 0, 0),
                event(EventKind::TaskReady, None, 20, 0, 0),
                event(EventKind::WorkerParked, Some(1), 600_000_000_020, 0, 0),
            ],
            lost_events: 1_000,
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source(None)), &[]);
        assert_eq!(snapshot.workers.len(), 1);
        let worker = &snapshot.workers[0];
        assert_eq!(worker.observed_tasks, 1);
        assert_eq!(worker.metrics.executing_fraction, None);
        assert_eq!(worker.tasks[0].activity, TaskActivitySummary::default());
        assert_eq!(worker.tasks[0].metrics.ready_samples.as_ref(), []);
    }

    #[test]
    fn overwritten_start_reconstructs_poll_but_underflow_is_unknown() {
        let events = Events {
            events: vec![
                event(EventKind::TaskPollFinished, Some(1), 100, 40, 0),
                event(EventKind::TaskPollFinished, Some(1), 200, 300, 0),
                event(EventKind::TaskPollFinished, Some(1), 200, 0, 0),
            ],
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, None, &[]);
        let metrics = &snapshot.workers[0].metrics;
        assert_eq!(
            (metrics.poll_count, metrics.median_poll_nanos, metrics.max_poll_nanos),
            (3, Some(40), Some(300))
        );
        assert_eq!(metrics.polls, [Interval { start: 60, end: 100 }, Interval { start: 200, end: 200 }]);
        assert_eq!(metrics.executing_fraction, Some(40.0 / 140.0));
    }

    #[test]
    fn legacy_zero_timestamps_do_not_create_an_idle_timeline() {
        let events = Events {
            events: vec![event(EventKind::TaskSpawned, None, 0, 0, 0)],
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source(None)), &[]);
        assert!(
            snapshot
                .workers
                .iter()
                .all(|worker| worker.window.is_none() && worker.metrics.executing_fraction.is_none())
        );
        assert_eq!(snapshot.workers[0].tasks[0].metrics.poll_count, 0);
    }

    #[test]
    fn queue_duration_samples_exclude_self_wake_execution_and_duplicate_wakes() {
        let events = Events {
            events: vec![
                event(EventKind::TaskPollStarted, Some(1), 10, 0, 0),
                event(EventKind::TaskReady, None, 20, 0, 0),
                event(EventKind::TaskReady, None, 20, 0, 0),
                event(EventKind::TaskPollFinished, Some(1), 80, 70, 0),
                event(EventKind::TaskPollStarted, Some(2), 100, 20, 2),
                // Legacy raw wake latency can include execution and is never called queue waiting.
                event(EventKind::TaskPollStarted, Some(2), 200, 150, 1),
            ],
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, None, &[]);
        assert_eq!(snapshot.workers.len(), 2);
        for worker in &snapshot.workers {
            assert_eq!(worker.tasks[0].metrics.ready_samples.as_ref(), [20]);
            assert_eq!(worker.tasks[0].metrics.ready.as_ref(), [Interval { start: 80, end: 100 }]);
            assert_eq!(worker.tasks[0].metrics.wake_samples.as_ref(), [150]);
        }
        assert_eq!(
            (snapshot.workers[0].metrics.poll_count, snapshot.workers[1].metrics.poll_count),
            (1, 0)
        );
    }

    #[test]
    fn legacy_running_placement_requires_an_exact_retained_poll_start() {
        let source = source(Some(TaskActivity {
            observed_at: EventTimestamp::from_ticks(100),
            state: TaskActivityState::Running,
            ready_since: None,
            poll_started_at: Some(EventTimestamp::from_ticks(10)),
            poll_worker_id: None,
            queued_since: None,
        }));
        for (start, expected) in [(10, Some(1.0)), (11, None)] {
            let events = Events {
                events: vec![event(EventKind::TaskPollStarted, Some(2), start, 0, 0)],
                ..Events::default()
            };
            let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source), &[]);
            assert_eq!(snapshot.workers[0].metrics.executing_fraction, None);
            assert_eq!(snapshot.workers[1].metrics.executing_fraction, expected);
            assert_eq!(snapshot.workers[1].tasks[0].activity.running_for, Some(90));
        }
    }

    #[test]
    fn conflicting_legacy_poll_start_placements_remain_unknown() {
        let source = source(Some(TaskActivity {
            observed_at: EventTimestamp::from_ticks(100),
            state: TaskActivityState::Running,
            ready_since: None,
            poll_started_at: Some(EventTimestamp::from_ticks(10)),
            poll_worker_id: None,
            queued_since: None,
        }));
        let events = Events {
            events: vec![
                event(EventKind::TaskPollStarted, Some(1), 10, 0, 0),
                event(EventKind::TaskPollStarted, Some(2), 10, 0, 0),
            ],
            ..Events::default()
        };
        let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source), &[]);
        assert!(snapshot.workers.iter().all(|worker| worker.metrics.executing_fraction.is_none()));
    }

    #[test]
    fn coherent_poll_worker_overrides_raced_worker_slot_without_a_retained_start() {
        let mut source = source(Some(TaskActivity {
            observed_at: EventTimestamp::from_ticks(100),
            state: TaskActivityState::Running,
            ready_since: None,
            poll_started_at: Some(EventTimestamp::from_ticks(10)),
            poll_worker_id: WorkerId::from_raw(1),
            queued_since: None,
        }));
        source.runtimes[0].workers[0].current_task = TaskId::from_raw(2);
        let snapshot = RuntimeMonitorSnapshot::from_events(&Events::default(), Some(&source), &[]);
        assert_eq!(snapshot.workers[0].metrics.executing_fraction, Some(1.0));
        assert_eq!(snapshot.workers[0].tasks[0].activity.running_for, Some(90));
    }

    #[test]
    fn display_window_rejects_zero_and_reversed_activity_ranges() {
        let mut source = source(Some(TaskActivity {
            observed_at: EventTimestamp::from_ticks(100),
            state: TaskActivityState::Running,
            ready_since: None,
            poll_started_at: Some(EventTimestamp::from_ticks(101)),
            poll_worker_id: WorkerId::from_raw(1),
            queued_since: None,
        }));
        assert!(display_windows(&Events::default(), Some(&source)).is_empty());
        source.runtimes[0].tasks[0].activity = Some(TaskActivity {
            observed_at: EventTimestamp::from_ticks(100),
            state: TaskActivityState::Running,
            ready_since: None,
            poll_started_at: Some(EventTimestamp::from_ticks(100)),
            poll_worker_id: WorkerId::from_raw(1),
            queued_since: None,
        });
        assert!(display_windows(&Events::default(), Some(&source)).is_empty());
    }
}
