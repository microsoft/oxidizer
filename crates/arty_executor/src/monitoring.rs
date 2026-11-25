// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
use seismograph_runtime::snapshot::{Task, TaskActivity, TaskActivityState, TaskMetrics, TaskMetricsScope};
use seismograph_runtime::task::{EventContext, TaskPoll, TaskSpawn};
use seismograph_runtime::worker::WorkerHandle;

use crate::TaskState;

static TYPE_DESCRIPTORS: OnceLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = OnceLock::new();

fn type_descriptor<T: 'static>() -> TypeDescriptorId {
    let mut descriptors = TYPE_DESCRIPTORS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *descriptors
        .entry(TypeId::of::<T>())
        .or_insert_with(seismograph_runtime::allocate_type_descriptor_id)
}

pub(crate) trait TaskMonitoring: 'static {
    fn id(&self) -> Option<TaskId>;

    fn poll_started(&mut self, _worker: &WorkerHandle) -> Option<TaskPoll> {
        None
    }

    fn poll_finished(&self, _worker: &WorkerHandle, _poll: TaskPoll) {}

    fn describe(&self, _state: TaskState) -> Option<Task> {
        None
    }

    fn completed(&mut self, _worker: Option<&WorkerHandle>) {}

    fn panicked(&mut self, _worker: Option<&WorkerHandle>) {}

    fn cancelled(&mut self, _worker: Option<&WorkerHandle>) {}
}

impl TaskMonitoring for () {
    fn id(&self) -> Option<TaskId> {
        None
    }
}

fn unrecorded_task(id: TaskId, state: TaskActivityState) -> Task {
    Task {
        id,
        parent: None,
        type_descriptor: TypeDescriptorId::unknown(),
        future_size_bytes: None,
        spawned_at: None,
        last_worker_id: None,
        metrics: TaskMetrics::default(),
        metrics_scope: TaskMetricsScope::Uncollected,
        activity: Some(TaskActivity {
            observed_at: EventTimestamp::now(),
            state,
            ready_since: None,
            poll_started_at: None,
            poll_worker_id: None,
            queued_since: None,
        }),
    }
}

/// Concrete metadata for one executor-owned task.
#[derive(Debug)]
pub struct TaskMetadata {
    events: EventContext,
    spawn: Option<TaskSpawn>,
    parent: Option<TaskId>,
    body_size: usize,
    describe_type: fn() -> TypeDescriptorId,
    identity: OnceCell<TaskId>,
}

impl TaskMetadata {
    /// Captures concrete task metadata and records a spawn event when enabled.
    #[must_use]
    pub fn capture<T: 'static>(events: EventContext, parent: Option<TaskId>, body_size: usize) -> Self {
        Self {
            events,
            spawn: events.spawned_with(type_descriptor::<T>, parent, body_size),
            parent,
            body_size,
            describe_type: type_descriptor::<T>,
            identity: OnceCell::new(),
        }
    }

    fn id(&self) -> TaskId {
        self.spawn.map_or_else(
            || *self.identity.get_or_init(seismograph_runtime::allocate_task_id),
            |spawn| spawn.id,
        )
    }

    fn describe(&self, state: TaskActivityState) -> Task {
        let mut task = Task::new(
            self.id(),
            self.spawn.map_or_else(|| (self.describe_type)(), |spawn| spawn.descriptor),
            state,
        );
        task.parent = self.parent;
        task.spawned_at = self.spawn.map(|spawn| spawn.timestamp);
        task.future_size_bytes = Some(u64::try_from(self.body_size).expect("a task body fits the platform address space"));
        task
    }

    fn terminal_id(&self) -> Option<TaskId> {
        self.spawn
            .map(|spawn| spawn.id)
            .or_else(|| seismograph::recorder::is_enabled(seismograph::recorder::event::EventClass::RuntimeTask).then(|| self.id()))
    }
}

#[derive(Debug)]
pub(crate) struct UnrecordedTask(TaskId);

#[derive(Debug)]
pub(crate) struct RecordedTask(Box<TaskMetadata>);

impl UnrecordedTask {
    pub(crate) fn new() -> Self {
        Self(seismograph_runtime::allocate_task_id())
    }
}

impl RecordedTask {
    pub(crate) fn new(task: TaskMetadata) -> Self {
        Self(Box::new(task))
    }

    fn task_id(&self) -> TaskId {
        self.0.id()
    }
}

impl TaskMonitoring for UnrecordedTask {
    fn id(&self) -> Option<TaskId> {
        Some(self.0)
    }

    fn describe(&self, state: TaskState) -> Option<Task> {
        let state = match state {
            TaskState::Ready => TaskActivityState::Ready,
            TaskState::Waiting => TaskActivityState::Waiting,
        };
        Some(unrecorded_task(self.0, state))
    }

    fn poll_started(&mut self, worker: &WorkerHandle) -> Option<TaskPoll> {
        Some(worker.task_poll_started(self.0))
    }

    fn poll_finished(&self, worker: &WorkerHandle, poll: TaskPoll) {
        worker.task_poll_finished(poll);
    }
}

impl TaskMonitoring for RecordedTask {
    fn id(&self) -> Option<TaskId> {
        Some(self.task_id())
    }

    fn describe(&self, state: TaskState) -> Option<Task> {
        let state = match state {
            TaskState::Ready => TaskActivityState::Ready,
            TaskState::Waiting => TaskActivityState::Waiting,
        };
        Some(self.0.describe(state))
    }

    fn poll_started(&mut self, worker: &WorkerHandle) -> Option<TaskPoll> {
        Some(worker.task_poll_started(self.task_id()))
    }

    fn poll_finished(&self, worker: &WorkerHandle, poll: TaskPoll) {
        worker.task_poll_finished(poll);
    }

    fn completed(&mut self, _worker: Option<&WorkerHandle>) {
        if let Some(task) = self.0.terminal_id() {
            self.0.events.completed(task);
        }
    }

    fn panicked(&mut self, _worker: Option<&WorkerHandle>) {
        if let Some(task) = self.0.terminal_id() {
            self.0.events.panicked(task);
        }
    }

    fn cancelled(&mut self, _worker: Option<&WorkerHandle>) {
        if let Some(task) = self.0.terminal_id() {
            self.0.events.canceled(task);
        }
    }
}
use std::any::TypeId;
use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
