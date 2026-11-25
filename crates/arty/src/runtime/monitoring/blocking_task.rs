// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::OnceLock;

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
use seismograph_runtime::snapshot::{Task, TaskActivityState};
use seismograph_runtime::task::{EventContext, TaskProperties};

use super::capture_task_properties;

#[derive(Debug)]
pub(crate) enum BlockingTaskInfo {
    Unrecorded(OnceLock<TaskId>),
    Recorded(Box<RecordedBlockingTask>),
}

#[derive(Debug)]
pub(crate) struct RecordedBlockingTask {
    properties: TaskProperties,
    runtime_events: EventContext,
    execution: OnceLock<ExecutionMetadata>,
}

impl RecordedBlockingTask {
    fn terminal_events(&self) -> EventContext {
        self.execution.get().map_or(self.runtime_events, |execution| execution.events)
    }
}

impl BlockingTaskInfo {
    pub(crate) fn capture<T: 'static>(events: EventContext) -> Self {
        let events = events.without_worker();
        events.spawned(|| capture_task_properties::<T>(size_of::<T>())).map_or_else(
            || Self::Unrecorded(OnceLock::new()),
            |properties| {
                Self::Recorded(Box::new(RecordedBlockingTask {
                    properties,
                    runtime_events: events,
                    execution: OnceLock::new(),
                }))
            },
        )
    }

    pub(crate) fn id(&self) -> TaskId {
        match self {
            Self::Unrecorded(id) => *id.get_or_init(seismograph_runtime::allocate_task_id),
            Self::Recorded(task) => task.properties.id(),
        }
    }

    pub(crate) fn describe(&self, state: TaskActivityState) -> Task {
        let execution = match self {
            Self::Recorded(info) => info.execution.get().copied(),
            Self::Unrecorded(_) => None,
        };
        let mut task = match self {
            Self::Unrecorded(id) => Task::new(
                *id.get_or_init(seismograph_runtime::allocate_task_id),
                TypeDescriptorId::unknown(),
                state,
            ),
            Self::Recorded(task) => task.properties.describe(state),
        };
        if state == TaskActivityState::Ready
            && let Some(spawned_at) = task.spawned_at
            && let Some(activity) = &mut task.activity
        {
            activity.ready_since = Some(spawned_at);
            activity.queued_since = Some(spawned_at);
        }
        if state == TaskActivityState::Running
            && let Some(execution) = execution
            && let Some(activity) = &mut task.activity
        {
            activity.poll_started_at = execution.started_at;
            activity.poll_worker_id = execution.events.worker_id();
            task.last_worker_id = execution.events.worker_id();
        }
        task
    }

    pub(crate) fn started_on(&self, events: EventContext, started_at: Option<EventTimestamp>) {
        if let Self::Recorded(info) = self {
            info.execution
                .set(ExecutionMetadata { events, started_at })
                .expect("a blocking function starts on exactly one worker");
        }
    }

    pub(crate) fn completed(&self) {
        self.record_outcome(EventContext::completed);
    }

    pub(crate) fn panicked(&self) {
        self.record_outcome(EventContext::panicked);
    }

    pub(crate) fn canceled(&self) {
        self.record_outcome(EventContext::canceled);
    }

    fn record_outcome(&self, outcome: impl FnOnce(&EventContext, TaskId)) {
        if let Self::Recorded(task) = self {
            outcome(&task.terminal_events(), task.properties.id());
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ExecutionMetadata {
    events: EventContext,
    started_at: Option<EventTimestamp>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use seismograph_runtime::task::TaskProperties;

    use super::BlockingTaskInfo;

    #[test]
    fn blocking_task_preserves_origin_and_execution_attribution() {
        use seismograph::recorder::event::{EventKind, EventTimestamp};
        use seismograph_runtime::snapshot::TaskActivityState;
        use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::Policy::all(false),
            ..seismograph::recorder::Configuration::default()
        });
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("blocking-attribution", 2));
        let origin = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let execution = runtime.register_worker(WorkerMetadata::new(WorkerRole::Blocking));
        let parent = seismograph_runtime::allocate_task_id();
        let info = {
            let _task = arty_executor::enter_task(parent, origin.handle().task_events());
            BlockingTaskInfo::capture::<()>(runtime.handle().task_events())
        };
        let started_at = EventTimestamp::from_ticks(37);
        info.started_on(execution.handle().task_events(), Some(started_at));

        let task = info.describe(TaskActivityState::Running);
        let activity = task.activity.unwrap();
        let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        seismograph::recorder(seismograph::recorder::Configuration::default());
        let spawn = snapshot
            .events
            .events
            .iter()
            .find(|event| {
                event.kind == EventKind::TaskSpawned && event.runtime().is_some_and(|payload| payload.subject_id == task.id.get())
            })
            .unwrap()
            .runtime()
            .unwrap();

        assert_eq!((spawn.worker_id, spawn.related_id), (Some(origin.id()), parent.get()));
        assert_eq!(
            (
                task.last_worker_id,
                activity.poll_worker_id,
                activity.poll_started_at,
                activity.state
            ),
            (
                Some(execution.id()),
                Some(execution.id()),
                Some(started_at),
                TaskActivityState::Running
            )
        );
    }

    #[test]
    fn absent_worker_events_preserve_plain_task_execution() {
        let mut executor = arty_executor::Executor::builder().build();
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let completed = std::rc::Rc::clone(&ran);
        let future = async move { completed.set(true) };
        let completion = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (_handle, sender) = completion.handle();
        let spawn = TaskProperties::new(seismograph_runtime::task::type_descriptor::<()>());

        executor.tasks().task(future).monitor(spawn).register(sender);
        let _ = executor.execute_cycle();

        assert!(ran.get());
    }
}
