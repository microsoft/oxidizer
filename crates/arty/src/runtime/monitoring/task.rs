// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use arty_executor::{TaskCompletion, TaskInstrumentation, TaskSet, TaskState};
use seismograph::recorder::event::{EventClass, EventTimestamp};
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
use seismograph_runtime::snapshot::{Task, TaskActivityState};
use seismograph_runtime::task::EventContext;

thread_local! {
    static EVENTS: Cell<Option<EventContext>> = const { Cell::new(None) };
    static CURRENT: Cell<Option<TaskId>> = const { Cell::new(None) };
    // This cache contains types, not live tasks. It is touched only during observation.
    static TYPES: RefCell<HashMap<TypeId, TypeDescriptorId>> = RefCell::new(HashMap::new());
}

pub(crate) fn current_events() -> Option<EventContext> {
    EVENTS.with(Cell::get)
}

pub(crate) fn set_events(events: Option<EventContext>) {
    EVENTS.with(|slot| slot.set(events));
}

pub(super) fn set_current(task: Option<TaskId>) {
    CURRENT.with(|slot| slot.set(task));
}

pub(crate) fn current_task() -> Option<TaskId> {
    CURRENT.with(Cell::get)
}

#[derive(Debug)]
#[must_use]
pub(crate) struct CurrentTask(Option<TaskId>);

impl Drop for CurrentTask {
    fn drop(&mut self) {
        // #[gamma::skip(call_result.default, reason = "set_current returns unit; default_after preserves the thread-local update.")]
        set_current(self.0);
    }
}

pub(crate) fn enter_current(task: TaskId) -> CurrentTask {
    CurrentTask(CURRENT.with(|slot| slot.replace(Some(task))))
}

pub(crate) struct PollTask(pub(crate) TaskId);

pub(crate) fn descriptor<T: 'static>() -> TypeDescriptorId {
    TYPES.with_borrow_mut(|types| {
        *types
            .entry(TypeId::of::<T>())
            .or_insert_with(seismograph_runtime::allocate_type_descriptor_id)
    })
}

#[derive(Debug)]
struct TaskMetadata {
    describe_type: fn() -> TypeDescriptorId,
    body_size: usize,
    events: EventContext,
    parent: Option<TaskId>,
    spawned_at: Option<EventTimestamp>,
}

impl TaskMetadata {
    fn new<T: 'static>(events: EventContext) -> Self {
        Self {
            describe_type: descriptor::<T>,
            body_size: size_of::<T>(),
            events,
            parent: current_task(),
            spawned_at: None,
        }
    }

    fn record_spawn(&mut self) -> Option<TaskIdentity> {
        self.events
            .spawned_with(self.describe_type, self.parent, self.body_size)
            .map(|spawn| {
                self.spawned_at = Some(spawn.timestamp);
                TaskIdentity {
                    id: spawn.id,
                    descriptor: spawn.descriptor,
                }
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
        task.parent = self.parent;
        task.spawned_at = self.spawned_at;
        task.future_size_bytes = Some(u64::try_from(self.body_size).expect("a task body fits the platform address space"));
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
    execution_events: OnceLock<EventContext>,
}

impl TaskInfo {
    pub(crate) fn new<T: 'static>(events: EventContext) -> Self {
        let mut metadata = TaskMetadata::new::<T>(events);
        let identity = metadata.record_spawn().map_or_else(OnceLock::new, OnceLock::from);
        Self {
            metadata,
            identity,
            execution_events: OnceLock::new(),
        }
    }

    pub(crate) fn id(&self) -> TaskId {
        self.identity.get_or_init(|| self.metadata.identity()).id
    }

    pub(crate) fn describe(&self, state: TaskActivityState) -> Task {
        self.metadata
            .describe(self.identity.get_or_init(|| self.metadata.identity()), state)
    }

    pub(crate) fn started_on(&self, events: EventContext) {
        self.execution_events
            .set(events)
            .expect("a blocking function starts on exactly one worker");
    }

    pub(crate) fn completed(&self) {
        if let Some(id) = self.terminal_id() {
            self.execution_events.get().unwrap_or(&self.metadata.events).completed(id);
        }
    }

    pub(crate) fn panicked(&self) {
        if let Some(id) = self.terminal_id() {
            self.execution_events.get().unwrap_or(&self.metadata.events).panicked(id);
        }
    }

    pub(crate) fn canceled(&self) {
        if let Some(id) = self.terminal_id() {
            self.execution_events.get().unwrap_or(&self.metadata.events).canceled(id);
        }
    }

    fn terminal_id(&self) -> Option<TaskId> {
        self.identity
            .get()
            .map(|identity| identity.id)
            .or_else(|| seismograph::recorder::is_enabled(EventClass::RuntimeTask).then(|| self.id()))
    }
}

// Async metadata stays in the executor's owner-only payload. Blocking metadata
// above remains synchronized because queue inspection and execution can overlap.
#[derive(Debug)]
struct AsyncTaskInfo {
    events: EventContext,
    parent: Option<TaskId>,
    spawned_at: Option<EventTimestamp>,
    identity: OnceCell<TaskId>,
}

impl AsyncTaskInfo {
    fn new(describe_type: fn() -> TypeDescriptorId, events: EventContext, parent: Option<TaskId>, body_size: usize) -> Self {
        let spawn = events.spawned_with(describe_type, parent, body_size);
        Self {
            events,
            parent,
            spawned_at: spawn.map(|spawn| spawn.timestamp),
            identity: spawn.map_or_else(OnceCell::new, |spawn| OnceCell::from(spawn.id)),
        }
    }

    fn id(&self) -> TaskId {
        *self.identity.get_or_init(seismograph_runtime::allocate_task_id)
    }

    fn describe(&self, descriptor: TypeDescriptorId, state: TaskActivityState, body_size: usize) -> Task {
        let mut task = Task::new(self.id(), descriptor, state);
        task.parent = self.parent;
        task.spawned_at = self.spawned_at;
        task.future_size_bytes = Some(u64::try_from(body_size).expect("a task body fits the platform address space"));
        task
    }

    fn terminal_id(&self) -> Option<TaskId> {
        self.identity
            .get()
            .copied()
            .or_else(|| seismograph::recorder::is_enabled(EventClass::RuntimeTask).then(|| self.id()))
    }
}

#[derive(Debug)]
struct TaskMonitor<I> {
    instrumentation: I,
    info: AsyncTaskInfo,
    describe_type: fn() -> TypeDescriptorId,
    body_size: usize,
}

impl<I> TaskInstrumentation for TaskMonitor<I>
where
    I: TaskInstrumentation,
{
    fn inspect(&self, state: TaskState, visitor: &mut dyn FnMut(&dyn std::any::Any)) {
        match state {
            TaskState::Ready | TaskState::Waiting => {
                let activity = if state == TaskState::Ready {
                    TaskActivityState::Ready
                } else {
                    TaskActivityState::Waiting
                };
                let task =
                    seismograph::recorder::without_recording(|| self.info.describe((self.describe_type)(), activity, self.body_size));
                visitor(&task);
            }
            TaskState::Running => {
                let task = seismograph::recorder::without_recording(|| PollTask(self.info.id()));
                visitor(&task);
            }
        }
    }

    fn poll_start(&mut self) {
        self.instrumentation.poll_start();
    }

    fn poll_end(&mut self, outcome: std::task::Poll<()>) {
        self.instrumentation.poll_end(outcome);
    }

    fn completed(&mut self) {
        if let Some(id) = self.info.terminal_id() {
            self.info.events.completed(id);
        }
        self.instrumentation.completed();
    }

    fn panicked(&mut self) {
        if let Some(id) = self.info.terminal_id() {
            self.info.events.panicked(id);
        }
        self.instrumentation.panicked();
    }

    fn cancelled(&mut self) {
        if let Some(id) = self.info.terminal_id() {
            self.info.events.canceled(id);
        }
        self.instrumentation.cancelled();
    }
}

pub(crate) fn register_monitored<F, T, I>(
    tasks: &TaskSet,
    future: F,
    completion: T,
    instrumentation: I,
    parent: Option<TaskId>,
    describe_type: fn() -> TypeDescriptorId,
    body_size: usize,
) where
    F: IntoFuture + 'static,
    F::Output: 'static,
    T: TaskCompletion<F::Output>,
    I: TaskInstrumentation,
{
    let Some(events) = current_events() else {
        tasks.task(future).instrument(instrumentation).register(completion);
        return;
    };
    let monitor = TaskMonitor {
        instrumentation,
        info: AsyncTaskInfo::new(describe_type, events, parent, body_size),
        describe_type,
        body_size,
    };
    tasks.task(future).instrument(monitor).register(completion);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use arty_executor::TaskInstrumentation;
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::{AsyncTaskInfo, TaskInfo, TaskMonitor, current_task, enter_current};

    struct Recorder(Rc<RefCell<Vec<&'static str>>>);

    impl TaskInstrumentation for Recorder {
        fn poll_start(&mut self) {
            self.0.borrow_mut().push("start");
        }

        fn poll_end(&mut self, _outcome: std::task::Poll<()>) {
            self.0.borrow_mut().push("end");
        }

        fn completed(&mut self) {
            self.0.borrow_mut().push("completed");
        }

        fn panicked(&mut self) {
            self.0.borrow_mut().push("panicked");
        }

        fn cancelled(&mut self) {
            self.0.borrow_mut().push("cancelled");
        }
    }

    #[test]
    fn task_monitor_forwards_instrumentation_callbacks() {
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("instrumentation-forwarding", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut monitor = TaskMonitor {
            instrumentation: Recorder(Rc::clone(&calls)),
            info: AsyncTaskInfo::new(super::descriptor::<()>, worker.handle().task_events(), None, 0),
            describe_type: super::descriptor::<()>,
            body_size: 0,
        };

        monitor.poll_start();
        monitor.poll_end(std::task::Poll::Pending);
        monitor.completed();
        monitor.panicked();
        monitor.cancelled();

        assert_eq!(*calls.borrow(), ["start", "end", "completed", "panicked", "cancelled"]);
    }

    #[test]
    fn current_task_scope_restores_its_parent_after_return_and_panic() {
        let parent = seismograph_runtime::allocate_task_id();
        let child = seismograph_runtime::allocate_task_id();
        assert_eq!(current_task(), None);
        {
            let _parent = enter_current(parent);
            {
                let _child = enter_current(child);
                assert_eq!(current_task(), Some(child));
            }
            assert_eq!(current_task(), Some(parent));
            let result = std::panic::catch_unwind(|| {
                let _child = enter_current(child);
                assert_eq!(current_task(), Some(child));
                panic!("scope restores the parent during unwinding");
            });
            assert!(result.is_err());
            assert_eq!(current_task(), Some(parent));
        }
        assert_eq!(current_task(), None);
    }

    #[test]
    fn async_metadata_is_smaller_and_stays_owner_local() {
        static_assertions::assert_not_impl_any!(AsyncTaskInfo: Sync);
        static_assertions::assert_impl_all!(TaskInfo: Send, Sync);
        assert!(size_of::<AsyncTaskInfo>() < size_of::<TaskInfo>());
        #[cfg(target_pointer_width = "64")]
        assert!(size_of::<AsyncTaskInfo>() <= 56);
    }

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
        super::set_events(None);
        let mut executor = arty_executor::Executor::builder().build();
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let completed = std::rc::Rc::clone(&ran);
        let future = async move { completed.set(true) };
        let completion = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (_handle, sender) = completion.handle();

        super::register_monitored(&executor.tasks(), future, sender, (), None, super::descriptor::<()>, 0);
        let _ = executor.execute_cycle();

        assert!(ran.get());
    }
}
