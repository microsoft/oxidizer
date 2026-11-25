// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::OnceLock;

use seismograph::recorder::event::{EventClass, EventTimestamp};
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
use seismograph_runtime::snapshot::{Task, TaskActivityState};
use seismograph_runtime::task::{EventContext, TaskProperties};

#[derive(Debug)]
struct TaskMetadata {
    describe_type: fn() -> TypeDescriptorId,
    properties: TaskProperties,
    spawn_events: EventContext,
    runtime_events: EventContext,
}

impl TaskMetadata {
    fn new<T: 'static>(events: EventContext) -> Self {
        let runtime_events = events.without_worker();
        let spawn_events = arty_executor::current_events()
            .filter(|current| current.runtime_id() == events.runtime_id())
            .unwrap_or(runtime_events);
        Self {
            describe_type: seismograph_runtime::task::type_descriptor::<T>,
            properties: TaskProperties::default()
                .parent(arty_executor::current_task())
                .body_size(size_of::<T>()),
            spawn_events,
            runtime_events,
        }
    }

    fn record_spawn(&mut self) -> Option<TaskIdentity> {
        let spawn = self.spawn_events.spawned(self.describe_type, &self.properties)?;
        self.properties = std::mem::take(&mut self.properties).spawned_at(Some(spawn.timestamp));
        Some(TaskIdentity {
            id: spawn.id,
            descriptor: spawn.descriptor,
        })
    }

    fn identity(&self) -> TaskIdentity {
        TaskIdentity {
            id: seismograph_runtime::allocate_task_id(),
            descriptor: (self.describe_type)(),
        }
    }

    fn describe(&self, identity: &TaskIdentity, state: TaskActivityState) -> Task {
        let mut task = Task::new(identity.id, identity.descriptor, state);
        self.properties.apply_to(&mut task);
        if state == TaskActivityState::Ready
            && let Some(spawned_at) = task.spawned_at
            && let Some(activity) = &mut task.activity
        {
            activity.ready_since = Some(spawned_at);
            activity.queued_since = Some(spawned_at);
        }
        task
    }
}

#[derive(Debug)]
struct TaskIdentity {
    id: TaskId,
    descriptor: TypeDescriptorId,
}

#[derive(Debug)]
pub(crate) struct TaskInfo {
    metadata: TaskMetadata,
    identity: OnceLock<TaskIdentity>,
    execution: OnceLock<ExecutionMetadata>,
}

impl TaskInfo {
    pub(crate) fn new<T: 'static>(events: EventContext) -> Self {
        let mut metadata = TaskMetadata::new::<T>(events);
        let identity = metadata.record_spawn().map_or_else(OnceLock::new, OnceLock::from);
        Self {
            metadata,
            identity,
            execution: OnceLock::new(),
        }
    }

    pub(crate) fn id(&self) -> TaskId {
        self.identity.get_or_init(|| self.metadata.identity()).id
    }

    pub(crate) fn describe(&self, state: TaskActivityState) -> Task {
        let mut task = self
            .metadata
            .describe(self.identity.get_or_init(|| self.metadata.identity()), state);
        if state == TaskActivityState::Running
            && let Some(execution) = self.execution.get()
            && let Some(activity) = &mut task.activity
        {
            activity.poll_started_at = execution.started_at;
            activity.poll_worker_id = execution.events.worker_id();
            task.last_worker_id = execution.events.worker_id();
        }
        task
    }

    pub(crate) fn started_on(&self, events: EventContext, started_at: Option<EventTimestamp>) {
        self.execution
            .set(ExecutionMetadata { events, started_at })
            .expect("a blocking function starts on exactly one worker");
    }

    pub(crate) fn completed(&self) {
        if let Some(id) = self.terminal_id() {
            self.execution
                .get()
                .map_or(&self.metadata.runtime_events, |execution| &execution.events)
                .completed(id);
        }
    }

    pub(crate) fn panicked(&self) {
        if let Some(id) = self.terminal_id() {
            self.execution
                .get()
                .map_or(&self.metadata.runtime_events, |execution| &execution.events)
                .panicked(id);
        }
    }

    pub(crate) fn canceled(&self) {
        if let Some(id) = self.terminal_id() {
            self.execution
                .get()
                .map_or(&self.metadata.runtime_events, |execution| &execution.events)
                .canceled(id);
        }
    }

    fn terminal_id(&self) -> Option<TaskId> {
        self.identity
            .get()
            .map(|identity| identity.id)
            .or_else(|| seismograph::recorder::is_enabled(EventClass::RuntimeTask).then(|| self.id()))
    }
}

#[derive(Debug)]
struct ExecutionMetadata {
    events: EventContext,
    started_at: Option<EventTimestamp>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::TaskInfo;

    #[test]
    fn unobserved_blocking_outcomes_do_not_allocate_task_identities() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("idle-outcomes", 1));
        let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
            seismograph_runtime::worker::WorkerRole::Blocking,
        ));
        seismograph::recorder::without_recording(|| {
            let info = TaskInfo::new::<()>(worker.handle().task_events());
            info.completed();
            info.panicked();
            info.canceled();
            assert!(info.identity.get().is_none());
        });
    }

    #[test]
    fn absent_worker_events_preserve_plain_task_execution() {
        let mut executor = arty_executor::Executor::builder().build();
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let completed = std::rc::Rc::clone(&ran);
        let future = async move { completed.set(true) };
        let completion = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (_handle, sender) = completion.handle();

        executor
            .tasks()
            .task(future)
            .monitor(arty_executor::TaskDescription::new::<()>(None))
            .register(sender);
        let _ = executor.execute_cycle();

        assert!(ran.get());
    }
}
