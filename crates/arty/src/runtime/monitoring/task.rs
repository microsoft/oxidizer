// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::sync::OnceLock;

use arty_executor::{TaskCompletion, TaskIdentity, TaskInstrumentation, TaskSet, TaskState};
use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::TaskId;
use seismograph_runtime::snapshot::{Task, TaskActivityState};
use seismograph_runtime::task::EventContext;

thread_local! {
    static EVENTS: Cell<Option<EventContext>> = const { Cell::new(None) };
}

pub(crate) fn current_events() -> Option<EventContext> {
    EVENTS.with(Cell::get)
}

pub(crate) fn set_events(events: Option<EventContext>) {
    EVENTS.with(|slot| slot.set(events));
}

#[derive(Debug)]
struct TaskMetadata {
    body_size: usize,
    events: EventContext,
    parent: Option<TaskId>,
    spawned_at: Option<EventTimestamp>,
}

impl TaskMetadata {
    fn new<T: 'static>(events: EventContext) -> Self {
        Self {
            body_size: size_of::<T>(),
            events,
            parent: crate::runtime::worker::asynchronous::LocalWorker::current_task().map(TaskIdentity::id),
            spawned_at: None,
        }
    }

    fn record_spawn(&mut self, identity: TaskIdentity) {
        self.spawned_at = self
            .events
            .spawned(identity.id(), identity.descriptor(), self.parent, self.body_size)
            .map(|spawn| spawn.timestamp);
    }

    fn describe(&self, identity: &TaskIdentity, state: TaskActivityState) -> Task {
        let mut task = Task::new(identity.id(), identity.descriptor(), state);
        task.parent = self.parent;
        task.spawned_at = self.spawned_at;
        task.future_size_bytes = Some(u64::try_from(self.body_size).expect("a task body fits the platform address space"));
        task
    }
}

#[derive(Debug)]
pub(crate) struct TaskInfo {
    metadata: TaskMetadata,
    identity: TaskIdentity,
    execution_events: OnceLock<EventContext>,
}

impl TaskInfo {
    pub(crate) fn new<T: 'static>(events: EventContext, identity: TaskIdentity) -> Self {
        let mut metadata = TaskMetadata::new::<T>(events);
        metadata.record_spawn(identity);
        Self {
            metadata,
            identity,
            execution_events: OnceLock::new(),
        }
    }

    pub(crate) fn id(&self) -> TaskId {
        self.identity.id()
    }

    pub(crate) const fn identity(&self) -> TaskIdentity {
        self.identity
    }

    pub(crate) fn describe(&self, state: TaskActivityState) -> Task {
        self.metadata.describe(&self.identity, state)
    }

    pub(crate) fn started_on(&self, events: EventContext) {
        self.execution_events
            .set(events)
            .expect("a blocking function starts on exactly one worker");
    }

    pub(crate) fn completed(&self) {
        self.execution_events.get().unwrap_or(&self.metadata.events).completed(self.id());
    }

    pub(crate) fn panicked(&self) {
        self.execution_events.get().unwrap_or(&self.metadata.events).panicked(self.id());
    }

    pub(crate) fn canceled(&self) {
        self.execution_events.get().unwrap_or(&self.metadata.events).canceled(self.id());
    }
}

// Async metadata stays in the executor's owner-only payload. Blocking metadata
// above remains synchronized because queue inspection and execution can overlap.
#[derive(Debug)]
struct AsyncTaskInfo {
    events: Option<EventContext>,
    parent: Option<TaskId>,
    spawned_at: Option<EventTimestamp>,
}

impl AsyncTaskInfo {
    fn new(identity: TaskIdentity, events: Option<EventContext>, parent: Option<TaskId>, body_size: usize) -> Self {
        let spawn = events.and_then(|events| events.spawned(identity.id(), identity.descriptor(), parent, body_size));
        Self {
            events,
            parent,
            spawned_at: spawn.map(|spawn| spawn.timestamp),
        }
    }

    fn describe(&self, identity: &TaskIdentity, state: TaskActivityState, body_size: usize) -> Task {
        let mut task = Task::new(identity.id(), identity.descriptor(), state);
        task.parent = self.parent;
        task.spawned_at = self.spawned_at;
        task.future_size_bytes = Some(u64::try_from(body_size).expect("a task body fits the platform address space"));
        task
    }
}

#[derive(Debug)]
struct TaskMonitor<I> {
    instrumentation: I,
    info: AsyncTaskInfo,
    body_size: usize,
}

impl<I> TaskInstrumentation for TaskMonitor<I>
where
    I: TaskInstrumentation,
{
    fn inspect(&self, identity: &TaskIdentity, state: TaskState, visitor: &mut dyn FnMut(&dyn std::any::Any)) {
        visitor(identity);
        match state {
            TaskState::Ready | TaskState::Waiting => {
                let activity = if state == TaskState::Ready {
                    TaskActivityState::Ready
                } else {
                    TaskActivityState::Waiting
                };
                let task = seismograph::recorder::without_recording(|| self.info.describe(identity, activity, self.body_size));
                visitor(&task);
            }
            TaskState::Running => {}
        }
    }

    fn poll_start(&mut self) {
        self.instrumentation.poll_start();
    }

    fn poll_end(&mut self, outcome: std::task::Poll<()>) {
        self.instrumentation.poll_end(outcome);
    }

    fn completed(&mut self, identity: &TaskIdentity) {
        if let Some(events) = self.info.events {
            events.completed(identity.id());
        }
        self.instrumentation.completed(identity);
    }

    fn panicked(&mut self, identity: &TaskIdentity) {
        if let Some(events) = self.info.events {
            events.panicked(identity.id());
        }
        self.instrumentation.panicked(identity);
    }

    fn cancelled(&mut self, identity: &TaskIdentity) {
        if let Some(events) = self.info.events {
            events.canceled(identity.id());
        }
        self.instrumentation.cancelled(identity);
    }
}

pub(crate) fn register_monitored<F, T, I>(
    tasks: &TaskSet,
    future: F,
    completion: T,
    instrumentation: I,
    identity: TaskIdentity,
    parent: Option<TaskId>,
    body_size: usize,
) where
    F: IntoFuture + 'static,
    F::Output: 'static,
    T: TaskCompletion<F::Output>,
    I: TaskInstrumentation,
{
    let monitor = TaskMonitor {
        instrumentation,
        info: AsyncTaskInfo::new(identity, current_events(), parent, body_size),
        body_size,
    };
    tasks.task(identity, future).instrument(monitor).register(completion);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use arty_executor::{TaskIdentity, TaskInstrumentation};
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::{AsyncTaskInfo, TaskInfo, TaskMonitor};

    struct Recorder(Rc<RefCell<Vec<&'static str>>>);

    impl TaskInstrumentation for Recorder {
        fn poll_start(&mut self) {
            self.0.borrow_mut().push("start");
        }

        fn poll_end(&mut self, _outcome: std::task::Poll<()>) {
            self.0.borrow_mut().push("end");
        }

        fn completed(&mut self, _identity: &TaskIdentity) {
            self.0.borrow_mut().push("completed");
        }

        fn panicked(&mut self, _identity: &TaskIdentity) {
            self.0.borrow_mut().push("panicked");
        }

        fn cancelled(&mut self, _identity: &TaskIdentity) {
            self.0.borrow_mut().push("cancelled");
        }
    }

    #[test]
    fn task_monitor_forwards_instrumentation_callbacks() {
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("instrumentation-forwarding", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let identity = TaskIdentity::from_location(std::panic::Location::caller());
        let mut monitor = TaskMonitor {
            instrumentation: Recorder(Rc::clone(&calls)),
            info: AsyncTaskInfo::new(identity, Some(worker.handle().task_events()), None, 0),
            body_size: 0,
        };

        monitor.poll_start();
        monitor.poll_end(std::task::Poll::Pending);
        monitor.completed(&identity);
        monitor.panicked(&identity);
        monitor.cancelled(&identity);

        assert_eq!(*calls.borrow(), ["start", "end", "completed", "panicked", "cancelled"]);
    }

    #[test]
    fn async_metadata_is_smaller_and_immutable() {
        static_assertions::assert_impl_all!(AsyncTaskInfo: Send, Sync);
        static_assertions::assert_impl_all!(TaskInfo: Send, Sync);
        assert!(size_of::<AsyncTaskInfo>() < size_of::<TaskInfo>());
        #[cfg(target_pointer_width = "64")]
        assert!(size_of::<AsyncTaskInfo>() <= 72);
    }

    #[test]
    fn unobserved_blocking_outcomes_preserve_the_submission_identity() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("idle-outcomes", 1));
        let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
            seismograph_runtime::worker::WorkerRole::Blocking,
        ));
        seismograph::recorder::without_recording(|| {
            let identity = TaskIdentity::from_location(std::panic::Location::caller());
            let info = TaskInfo::new::<()>(worker.handle().task_events(), identity);
            info.completed();
            info.panicked();
            info.canceled();
            assert_eq!(info.identity(), identity);
        });
    }

    #[test]
    fn absent_worker_events_preserve_plain_task_execution() {
        super::set_events(None);
        let mut executor = arty_executor::Executor::builder().build();
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let completed = std::rc::Rc::clone(&ran);
        let future = async move { completed.set(true) };
        let completion = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (_handle, sender) = completion.handle();

        let identity = TaskIdentity::from_location(std::panic::Location::caller());
        super::register_monitored(&executor.tasks(), future, sender, (), identity, None, 0);
        let mut identities = Vec::new();
        executor.inspect(1, |identity: &TaskIdentity| identities.push(*identity)).unwrap();
        assert_eq!(identities, [identity]);
        let _ = executor.execute_cycle();

        assert!(ran.get());
    }
}
