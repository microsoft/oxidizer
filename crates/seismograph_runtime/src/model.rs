// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Logical runtime, worker, and task state reconstructed from retained data.

use std::collections::{BTreeMap, BTreeSet};

use seismograph::recorder::event::{EventKind, EventTimestamp, Events};
use seismograph::recorder::runtime::{RuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph::recorder::thread::ThreadId;

use crate::snapshot::{RuntimeState, Snapshot, WorkerState};
use crate::{TaskOutcome, WorkerRole};

/// Logical runtime state reconstructed from a source snapshot and retained events.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct RuntimeModel {
    /// Runtimes visible in either retained input.
    pub runtimes: Vec<Runtime>,
}

impl RuntimeModel {
    /// Reconstructs runtime state from retained events and optional source rows.
    #[must_use]
    pub fn from_events(events: &Events, source: Option<&Snapshot>) -> Self {
        let mut runtimes = BTreeMap::<RuntimeId, RuntimeBuilder>::new();
        if let Some(source) = source {
            for runtime in &source.runtimes {
                let entry = runtimes.entry(runtime.id).or_insert_with(|| RuntimeBuilder::new(runtime.id));
                entry.runtime.name.clone_from(&runtime.name);
                entry.runtime.configured_workers = Some(runtime.configured_workers);
                entry.runtime.state = runtime.state;
                entry.state_from_source = true;
                for worker in &runtime.workers {
                    entry.workers.insert(
                        worker.id,
                        WorkerBuilder {
                            worker: Worker {
                                id: worker.id,
                                role: Some(worker.role),
                                state: worker.state,
                                processor_index: worker.processor_index,
                                thread_id: worker.thread_id,
                                current_task: None,
                                tasks: Vec::new(),
                            },
                            state_from_source: true,
                            tasks: BTreeSet::new(),
                        },
                    );
                }
            }
        }
        let mut ordered = events.events.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|event| (event.timestamp, event.thread_id, event.sequence));
        for event in ordered {
            let Some(payload) = event.runtime() else { continue };
            let runtime = runtimes
                .entry(payload.runtime_id)
                .or_insert_with(|| RuntimeBuilder::new(payload.runtime_id));
            runtime.observe_runtime(event.kind, payload.value_0);
            let worker_id = payload.worker_id.filter(|_| event.kind != EventKind::TaskReady);
            if let Some(worker_id) = worker_id {
                runtime.observe_worker(
                    worker_id,
                    event.kind,
                    event.thread_id,
                    payload.subject_id,
                    payload.value_0,
                    payload.value_1,
                );
            }
            let Some(task_id) = task_id(event.kind, payload.subject_id, payload.related_id) else {
                continue;
            };
            let task = runtime
                .tasks
                .entry(task_id)
                .or_insert_with(|| TaskBuilder::new(payload.runtime_id, task_id));
            if let Some(worker) =
                worker_id.filter(|_| !matches!(event.kind, EventKind::TaskSpawned | EventKind::TaskEnqueued | EventKind::TaskReady))
            {
                task.workers.insert(worker);
            }
            let worker_role = worker_id.and_then(|worker| runtime.workers.get(&worker).and_then(|worker| worker.worker.role));
            task.observe(
                event.kind,
                event.timestamp,
                worker_id,
                worker_role,
                payload.related_id,
                payload.value_0,
                payload.value_1,
            );
            runtime.update_current_task(event.kind, worker_id, task_id);
        }
        Self {
            runtimes: runtimes.into_values().map(RuntimeBuilder::finish).collect(),
        }
    }
}

/// One logical runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Runtime {
    /// Runtime identity.
    pub id: RuntimeId,
    /// Runtime name, or a generated fallback when no source row is retained.
    pub name: String,
    /// Configured asynchronous worker count, when retained.
    pub configured_workers: Option<u32>,
    /// Lifecycle state at the end of the retained data.
    pub state: RuntimeState,
    /// Workers visible in retained data.
    pub workers: Vec<Worker>,
    /// Tasks visible in retained events.
    pub tasks: Vec<Task>,
}

/// One logical worker.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Worker {
    /// Worker identity.
    pub id: WorkerId,
    /// Worker role, when retained.
    pub role: Option<WorkerRole>,
    /// Worker state at the end of the retained data.
    pub state: WorkerState,
    /// Configured processor, when retained.
    pub processor_index: Option<u32>,
    /// Recorder thread, when retained.
    pub thread_id: Option<ThreadId>,
    /// Task whose retained poll has not finished.
    pub current_task: Option<TaskId>,
    /// Tasks associated with this worker.
    pub tasks: Vec<TaskId>,
}

/// One logical task reconstructed from retained events.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Task {
    /// Runtime that owns this task.
    pub runtime_id: RuntimeId,
    /// Process-monotonic task identity.
    pub id: TaskId,
    /// Parent task, when its queue event is retained.
    pub parent: Option<TaskId>,
    /// Spawn descriptor, when its queue event is retained.
    pub type_descriptor: Option<TypeDescriptorId>,
    /// Concrete body size, when its first-execution event is retained.
    pub body_size: Option<u64>,
    /// Worker that submitted the task, when retained.
    pub origin_worker: Option<WorkerId>,
    /// Workers that executed or reported this task.
    pub workers: Vec<WorkerId>,
    /// Current state at the end of the retained event ring.
    pub state: TaskState,
    /// Task-specific lifecycle model.
    pub kind: TaskKind,
}

/// Task lifecycle category inferred from retained events.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum TaskKind {
    /// The retained events do not distinguish the task category.
    #[default]
    Unknown,
    /// An asynchronous task with poll history.
    Asynchronous(AsynchronousTask),
    /// A blocking task with execution history.
    Blocking(BlockingTask),
}

/// State at the end of the retained event ring.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum TaskState {
    /// The retained ring does not establish a state.
    #[default]
    Unknown,
    /// The task is queued.
    Queued,
    /// The task is executing or polling.
    Running,
    /// The asynchronous task is waiting for another poll.
    Waiting,
    /// The task completed.
    Completed,
    /// The task was canceled.
    Cancelled,
    /// The task panicked.
    Panicked,
}

/// Asynchronous task history.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct AsynchronousTask {
    /// Retained completed or active polls.
    pub polls: Vec<TaskPoll>,
    /// Terminal outcome, when retained.
    pub outcome: Option<TaskOutcome>,
}

/// One retained asynchronous poll.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TaskPoll {
    /// Worker that executed the poll, when retained.
    pub worker: Option<WorkerId>,
    /// Poll start, when retained or inferred from duration.
    pub started_at: Option<EventTimestamp>,
    /// Poll finish, when retained.
    pub finished_at: Option<EventTimestamp>,
    /// Poll duration in nanoseconds, when the finish event is retained.
    pub duration_nanos: Option<u64>,
    /// Terminal outcome carried by the finish event.
    pub outcome: Option<TaskOutcome>,
}

/// Blocking task history.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct BlockingTask {
    /// Execution start, when retained.
    pub started_at: Option<EventTimestamp>,
    /// Execution finish, when retained.
    pub finished_at: Option<EventTimestamp>,
    /// Execution worker, when retained.
    pub worker: Option<WorkerId>,
    /// Execution duration in nanoseconds, when retained.
    pub duration_nanos: Option<u64>,
    /// Terminal outcome, when retained.
    pub outcome: Option<TaskOutcome>,
}

struct RuntimeBuilder {
    runtime: Runtime,
    workers: BTreeMap<WorkerId, WorkerBuilder>,
    tasks: BTreeMap<TaskId, TaskBuilder>,
    state_from_source: bool,
}

impl RuntimeBuilder {
    fn new(id: RuntimeId) -> Self {
        Self {
            runtime: Runtime {
                id,
                name: format!("runtime #{}", id.get()),
                configured_workers: None,
                state: RuntimeState::Running,
                workers: Vec::new(),
                tasks: Vec::new(),
            },
            workers: BTreeMap::new(),
            tasks: BTreeMap::new(),
            state_from_source: false,
        }
    }

    fn observe_runtime(&mut self, kind: EventKind, value_0: u64) {
        if self.state_from_source {
            return;
        }
        match kind {
            EventKind::RuntimeCreated => {
                self.runtime.configured_workers = u32::try_from(value_0).ok();
                self.runtime.state = RuntimeState::Running;
            }
            EventKind::RuntimeStopping => self.runtime.state = RuntimeState::Stopping,
            EventKind::RuntimeStopped => self.runtime.state = RuntimeState::Stopped,
            _ => {}
        }
    }

    fn observe_worker(&mut self, id: WorkerId, kind: EventKind, thread_id: ThreadId, subject: u64, value_0: u64, value_1: u64) {
        let worker = self.workers.entry(id).or_insert_with(|| WorkerBuilder {
            worker: Worker {
                id,
                role: None,
                state: WorkerState::Running,
                processor_index: None,
                thread_id: None,
                current_task: None,
                tasks: Vec::new(),
            },
            state_from_source: false,
            tasks: BTreeSet::new(),
        });
        let update_thread = matches!(
            kind,
            EventKind::WorkerStarted
                | EventKind::WorkerParked
                | EventKind::WorkerUnparked
                | EventKind::WorkerStopped
                | EventKind::TaskMaterialized
                | EventKind::TaskPollStarted
                | EventKind::TaskPollFinished
        );
        if update_thread && worker.worker.thread_id.is_none() {
            worker.worker.thread_id = Some(thread_id);
        }
        match kind {
            EventKind::WorkerStarted => {
                worker.worker.role = u8::try_from(value_0).ok().and_then(WorkerRole::from_wire_value);
                worker.worker.processor_index = u32::try_from(value_1).ok().filter(|value| *value != u32::MAX);
                if !worker.state_from_source {
                    worker.worker.state = WorkerState::Running;
                }
            }
            EventKind::WorkerParked if !worker.state_from_source => worker.worker.state = WorkerState::Parked,
            EventKind::WorkerUnparked if !worker.state_from_source => worker.worker.state = WorkerState::Running,
            EventKind::WorkerStopped => worker.worker.state = WorkerState::Stopped,
            _ => {
                let _ = subject;
            }
        }
    }

    fn update_current_task(&mut self, kind: EventKind, worker: Option<WorkerId>, task: TaskId) {
        let Some(worker_id) = worker else { return };
        let worker = self.workers.entry(worker_id).or_insert_with(|| WorkerBuilder {
            worker: Worker {
                id: worker_id,
                role: None,
                state: WorkerState::Running,
                processor_index: None,
                thread_id: None,
                current_task: None,
                tasks: Vec::new(),
            },
            state_from_source: false,
            tasks: BTreeSet::new(),
        });
        if worker.tasks.insert(task) {
            worker.worker.tasks.push(task);
        }
        match kind {
            EventKind::TaskPollStarted => worker.worker.current_task = Some(task),
            EventKind::TaskMaterialized if worker.worker.role == Some(WorkerRole::Blocking) => {
                worker.worker.current_task = Some(task);
            }
            EventKind::TaskPollFinished | EventKind::TaskCompleted | EventKind::TaskCanceled | EventKind::TaskPanicked
                if worker.worker.current_task == Some(task) =>
            {
                worker.worker.current_task = None;
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Runtime {
        self.runtime.workers = self.workers.into_values().map(|worker| worker.worker).collect();
        self.runtime.tasks = self
            .tasks
            .into_values()
            .map(|mut task| {
                task.task.workers = task.workers.into_iter().collect();
                task.task
            })
            .collect();
        self.runtime
    }
}

struct WorkerBuilder {
    worker: Worker,
    state_from_source: bool,
    tasks: BTreeSet<TaskId>,
}

struct TaskBuilder {
    task: Task,
    workers: BTreeSet<WorkerId>,
}

impl TaskBuilder {
    fn new(runtime_id: RuntimeId, id: TaskId) -> Self {
        Self {
            task: Task {
                runtime_id,
                id,
                parent: None,
                type_descriptor: None,
                body_size: None,
                origin_worker: None,
                workers: Vec::new(),
                state: TaskState::Unknown,
                kind: TaskKind::Unknown,
            },
            workers: BTreeSet::new(),
        }
    }

    fn observe(
        &mut self,
        kind: EventKind,
        timestamp: EventTimestamp,
        worker: Option<WorkerId>,
        worker_role: Option<WorkerRole>,
        related: u64,
        value_0: u64,
        value_1: u64,
    ) {
        match kind {
            EventKind::TaskSpawned | EventKind::TaskEnqueued => {
                self.task.parent = TaskId::from_raw(related);
                self.task.type_descriptor = TypeDescriptorId::from_raw(value_0);
                if kind == EventKind::TaskSpawned {
                    self.task.body_size = value_1.checked_sub(1);
                }
                self.task.origin_worker = worker;
                self.task.state = TaskState::Queued;
            }
            EventKind::TaskMaterialized => {
                self.task.body_size = value_1.checked_sub(1);
                self.task.state = TaskState::Running;
                if worker_role == Some(WorkerRole::Blocking) {
                    let task = self.blocking();
                    task.worker = worker;
                    task.started_at = Some(timestamp);
                }
            }
            EventKind::TaskPollStarted => {
                self.task.state = TaskState::Running;
                let task = self.asynchronous();
                task.polls.push(TaskPoll {
                    worker,
                    started_at: Some(timestamp),
                    finished_at: None,
                    duration_nanos: None,
                    outcome: None,
                });
            }
            EventKind::TaskPollFinished => {
                let outcome = poll_outcome(value_1);
                self.task.state = outcome.as_ref().map_or(TaskState::Waiting, state_from_outcome);
                let task = self.asynchronous();
                task.outcome.clone_from(&outcome);
                if let Some(poll) = task.polls.last_mut().filter(|poll| poll.finished_at.is_none()) {
                    poll.worker = poll.worker.or(worker);
                    poll.finished_at = Some(timestamp);
                    poll.duration_nanos = Some(value_0);
                    poll.outcome = outcome;
                } else {
                    task.polls.push(TaskPoll {
                        worker,
                        started_at: timestamp.ticks().checked_sub(value_0).map(EventTimestamp::from_ticks),
                        finished_at: Some(timestamp),
                        duration_nanos: Some(value_0),
                        outcome,
                    });
                }
            }
            EventKind::TaskCompleted | EventKind::TaskPanicked => {
                let outcome = if kind == EventKind::TaskCompleted {
                    TaskOutcome::Completed
                } else {
                    TaskOutcome::Panicked
                };
                self.task.state = state_from_outcome(&outcome);
                let task = self.blocking();
                task.worker = worker;
                task.finished_at = Some(timestamp);
                task.duration_nanos = Some(value_0);
                task.started_at = timestamp.ticks().checked_sub(value_0).map(EventTimestamp::from_ticks);
                task.outcome = Some(outcome);
            }
            EventKind::TaskCanceled => {
                self.task.state = TaskState::Cancelled;
                match &mut self.task.kind {
                    TaskKind::Asynchronous(task) => task.outcome = Some(TaskOutcome::Cancelled),
                    TaskKind::Blocking(task) => {
                        task.worker = worker;
                        task.finished_at = Some(timestamp);
                        task.duration_nanos = Some(value_0);
                        task.started_at = timestamp.ticks().checked_sub(value_0).map(EventTimestamp::from_ticks);
                        task.outcome = Some(TaskOutcome::Cancelled);
                    }
                    TaskKind::Unknown if worker_role == Some(WorkerRole::Blocking) => {
                        let task = self.blocking();
                        task.worker = worker;
                        task.finished_at = Some(timestamp);
                        task.duration_nanos = Some(value_0);
                        task.started_at = timestamp.ticks().checked_sub(value_0).map(EventTimestamp::from_ticks);
                        task.outcome = Some(TaskOutcome::Cancelled);
                    }
                    TaskKind::Unknown => {}
                }
            }
            _ => {}
        }
    }

    fn asynchronous(&mut self) -> &mut AsynchronousTask {
        if !matches!(self.task.kind, TaskKind::Asynchronous(_)) {
            self.task.kind = TaskKind::Asynchronous(AsynchronousTask::default());
        }
        let TaskKind::Asynchronous(task) = &mut self.task.kind else {
            unreachable!("the task kind was set above");
        };
        task
    }

    fn blocking(&mut self) -> &mut BlockingTask {
        if !matches!(self.task.kind, TaskKind::Blocking(_)) {
            self.task.kind = TaskKind::Blocking(BlockingTask::default());
        }
        let TaskKind::Blocking(task) = &mut self.task.kind else {
            unreachable!("the task kind was set above");
        };
        task
    }
}

fn poll_outcome(value: u64) -> Option<TaskOutcome> {
    match value {
        1 => Some(TaskOutcome::Completed),
        2 => Some(TaskOutcome::Cancelled),
        3 => Some(TaskOutcome::Panicked),
        _ => None,
    }
}

fn state_from_outcome(outcome: &TaskOutcome) -> TaskState {
    match outcome {
        TaskOutcome::Completed => TaskState::Completed,
        TaskOutcome::Cancelled => TaskState::Cancelled,
        TaskOutcome::Panicked => TaskState::Panicked,
    }
}

fn task_id(kind: EventKind, subject: u64, related: u64) -> Option<TaskId> {
    let raw = match kind {
        EventKind::TaskSpawned
        | EventKind::TaskEnqueued
        | EventKind::TaskMaterialized
        | EventKind::TaskPollStarted
        | EventKind::TaskPollFinished
        | EventKind::TaskCompleted
        | EventKind::TaskCanceled
        | EventKind::TaskPanicked => subject,
        EventKind::TaskReady | EventKind::TransferStarted | EventKind::TransferFinished => related,
        EventKind::InstanceRelocated => subject,
        _ => return None,
    };
    TaskId::from_raw(raw)
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{Event, EventPayload, EventSequence};
    use seismograph::recorder::runtime::RuntimeEvent;

    use super::*;

    fn event(kind: EventKind, task: u64, worker: Option<u64>, value_0: u64, value_1: u64) -> Event {
        Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(100),
            kind,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: worker.and_then(WorkerId::from_raw),
                subject_id: task,
                related_id: 0,
                value_0,
                value_1,
            }),
            call_stack: Vec::new(),
        }
    }

    #[test]
    fn poll_only_history_creates_an_incomplete_asynchronous_task() {
        let events = Events {
            events: vec![
                event(EventKind::TaskPollStarted, 7, Some(3), 0, 0),
                event(EventKind::TaskPollFinished, 7, Some(3), 11, 0),
            ],
            ..Events::default()
        };

        let model = RuntimeModel::from_events(&events, None);
        assert_eq!(model.runtimes[0].tasks[0].state, TaskState::Waiting);
        assert!(model.runtimes[0].tasks[0].parent.is_none());
        let TaskKind::Asynchronous(task) = &model.runtimes[0].tasks[0].kind else {
            panic!("poll events identify an asynchronous task");
        };
        assert_eq!(task.polls[0].duration_nanos, Some(11));
        assert_eq!(model.runtimes[0].workers[0].current_task, None);
    }
}
