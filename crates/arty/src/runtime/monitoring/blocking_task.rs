// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::OnceLock;

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
use seismograph_runtime::RuntimeHandle;
use seismograph_runtime::snapshot::{Task, TaskActivity, TaskActivityState, TaskMetrics, TaskMetricsScope};
use seismograph_runtime::task::{EventContext, TaskPoll, TaskSpawn};
use seismograph_runtime::worker::WorkerHandle;

use super::capture_task_properties;

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

#[derive(Debug)]
pub(crate) enum BlockingTaskInfo {
    Unrecorded(OnceLock<TaskId>),
    Recorded(Box<RecordedBlockingTask>),
}

#[derive(Debug)]
pub(crate) struct RecordedBlockingTask {
    task: TaskSpawn,
    runtime_events: EventContext,
    execution: OnceLock<ExecutionMetadata>,
}

impl RecordedBlockingTask {
    fn terminal_events(&self) -> EventContext {
        self.execution
            .get()
            .map_or(self.runtime_events, |execution| execution.worker.task_events())
    }
}

#[derive(Debug)]
struct ExecutionMetadata {
    worker: WorkerHandle,
    started_at: Option<EventTimestamp>,
}

impl BlockingTaskInfo {
    pub(crate) fn capture<T: 'static>(runtime: &RuntimeHandle) -> Self {
        let events = runtime.task_events();
        events.spawned(|| capture_task_properties::<T>(size_of::<T>())).map_or_else(
            || Self::Unrecorded(OnceLock::new()),
            |properties| {
                Self::Recorded(Box::new(RecordedBlockingTask {
                    task: properties,
                    runtime_events: events,
                    execution: OnceLock::new(),
                }))
            },
        )
    }

    pub(crate) fn id(&self) -> TaskId {
        match self {
            Self::Unrecorded(id) => *id.get_or_init(seismograph_runtime::allocate_task_id),
            Self::Recorded(task) => task.task.id(),
        }
    }

    pub(crate) fn describe(&self, state: TaskActivityState) -> Task {
        let execution = match self {
            Self::Recorded(info) => info.execution.get(),
            Self::Unrecorded(_) => None,
        };
        let mut task = match self {
            Self::Unrecorded(id) => unrecorded_task(*id.get_or_init(seismograph_runtime::allocate_task_id), state),
            Self::Recorded(task) => task.task.describe(state),
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
            activity.poll_worker_id = Some(execution.worker.id());
            task.last_worker_id = Some(execution.worker.id());
        }
        task
    }

    pub(crate) fn started_on(&self, worker: &WorkerHandle, observed: bool) -> Option<TaskPoll> {
        match self {
            Self::Unrecorded(_) if observed => Some(worker.task_poll_started(self.id())),
            Self::Unrecorded(_) => None,
            Self::Recorded(info) => {
                let poll = observed.then(|| worker.task_poll_started(info.task.id()));
                info.execution
                    .set(ExecutionMetadata {
                        worker: worker.clone(),
                        started_at: poll.as_ref().and_then(TaskPoll::started_at),
                    })
                    .expect("a blocking function starts on exactly one worker");
                poll
            }
        }
    }

    pub(crate) fn poll_finished(&self, worker: &WorkerHandle, poll: TaskPoll) {
        match self {
            Self::Unrecorded(_) | Self::Recorded(_) => worker.task_poll_finished(poll),
        }
    }

    pub(crate) fn completed(&self) {
        if let Self::Recorded(task) = self {
            task.terminal_events().completed(task.task.id());
        }
    }

    pub(crate) fn panicked(&self) {
        if let Self::Recorded(task) = self {
            task.terminal_events().panicked(task.task.id());
        }
    }

    pub(crate) fn canceled(&self) {
        if let Self::Recorded(task) = self {
            task.terminal_events().canceled(task.task.id());
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
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
            let _task = arty_executor::enter_task(parent);
            BlockingTaskInfo::capture::<()>(&runtime.handle())
        };
        let started_at = EventTimestamp::from_ticks(37);
        info.started_on(&execution.handle(), Some(started_at));

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
        executor.tasks().task(future).monitor(None).register(sender);
        let _ = executor.execute_cycle();

        assert!(ran.get());
    }
}
