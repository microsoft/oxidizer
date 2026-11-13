// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use arty_executor::{ShutdownCause, TaskCompletion, TaskPanicPayload, TaskPlacement, TaskSet, TaskState};
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

pub(super) fn set_events(events: Option<EventContext>) {
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
        set_current(self.0);
    }
}

pub(crate) fn enter_current(task: TaskId) -> CurrentTask {
    CurrentTask(CURRENT.with(|slot| slot.replace(Some(task))))
}

#[expect(
    clippy::large_enum_variant,
    reason = "descriptions are transient stack values; boxing snapshots would allocate during native inspection"
)]
pub(super) enum Description {
    Snapshot(Task),
    Poll(TaskId),
}

fn descriptor<T: 'static>() -> TypeDescriptorId {
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
    fn new<T: 'static>(events: EventContext, parent: Option<TaskId>, body_size: usize) -> Self {
        let spawn = events.spawned_with(descriptor::<T>, parent, body_size);
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

    fn describe<T: 'static>(&self, state: TaskActivityState, body_size: usize) -> Task {
        let mut task = Task::new(self.id(), descriptor::<T>(), state);
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
struct Completion<T> {
    target: T,
    info: AsyncTaskInfo,
}

impl<R: 'static, T: TaskCompletion<R>> TaskCompletion<R> for Completion<T> {
    fn completed(self, result: R) {
        if let Some(id) = self.info.terminal_id() {
            self.info.events.completed(id);
        }
        self.target.completed(result);
    }

    fn panicked(self, panic: TaskPanicPayload, cause: Option<ShutdownCause>) {
        if let Some(id) = self.info.terminal_id() {
            self.info.events.panicked(id);
        }
        self.target.panicked(panic, cause);
    }

    fn cancelled(self, cause: ShutdownCause) {
        if let Some(id) = self.info.terminal_id() {
            self.info.events.canceled(id);
        }
        self.target.cancelled(cause);
    }
}

pub(crate) fn add_task<Body, F, R, T>(tasks: &TaskSet, future: F, placement: TaskPlacement, completion: T)
where
    Body: 'static,
    F: IntoFuture<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    // The function item stores no bytes. Only erased scoped futures carry a size.
    add_task_with_metadata::<Body, _, _, _, _>(tasks, future, placement, completion, current_task(), size_of::<Body>);
}

pub(crate) fn add_task_with_metadata<Body, F, R, T, S>(
    tasks: &TaskSet,
    future: F,
    placement: TaskPlacement,
    completion: T,
    parent: Option<TaskId>,
    body_size: S,
) where
    Body: 'static,
    F: IntoFuture<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    S: Fn() -> usize + 'static,
{
    let Some(events) = current_events() else {
        tasks.task(future).placement(placement).register(completion);
        return;
    };
    let completion = Completion {
        target: completion,
        info: AsyncTaskInfo::new::<Body>(events, parent, body_size()),
    };
    tasks
        .task(future)
        .placement(placement)
        .register_with_inspection(completion, move |_, completion, state| {
            let info = &completion.info;
            Some(seismograph::recorder::without_recording(|| match state {
                TaskState::Ready => Description::Snapshot(info.describe::<Body>(TaskActivityState::Ready, body_size())),
                TaskState::Waiting => Description::Snapshot(info.describe::<Body>(TaskActivityState::Waiting, body_size())),
                TaskState::Running => Description::Poll(info.id()),
            }))
        });
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use arty_executor::{ShutdownCause, ShutdownError, TaskCompletion};
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::{AsyncTaskInfo, Completion, TaskInfo, current_task, enter_current};
    use crate::runtime::shutdown::{Shutdown, ShutdownState};
    use crate::task::handle::JoinHandlePool;

    #[test]
    fn monitored_panic_preserves_the_payload_and_executor_shutdown_cause() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("panic-forwarding", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));

        for stopping in [false, true] {
            let pool = JoinHandlePool::new(Shutdown::default());
            let (handle, target) = pool.handle::<u32>();
            let cause = stopping.then(|| ShutdownCause::panic(&42_u32));
            let expected = cause.as_ref().and_then(ShutdownCause::as_error).cloned();
            let completion = Completion {
                target,
                info: AsyncTaskInfo::new::<()>(worker.handle().task_events(), None, 0),
            };

            completion.panicked(Box::new(42_u32), cause);

            if let Some(expected) = expected {
                assert!(std::ptr::addr_eq(pool.shutdown().error().unwrap().inner(), expected.inner()));
            } else {
                assert_eq!(pool.shutdown().state(), ShutdownState::Running);
            }
            assert_eq!(*handle.join().unwrap_err().into_panic().unwrap().downcast::<u32>().unwrap(), 42);
        }
    }

    #[test]
    fn monitored_cancellation_delivers_the_executor_shutdown_cause() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("cancellation-forwarding", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let pool = JoinHandlePool::new(Shutdown::default());
        let (handle, target) = pool.handle::<u32>();
        let cause = ShutdownCause::panic(&"executor failure");
        let expected = cause.as_error().unwrap().clone();
        let completion = Completion {
            target,
            info: AsyncTaskInfo::new::<()>(worker.handle().task_events(), None, 0),
        };

        completion.cancelled(cause);
        let error = handle.join().unwrap_err();

        assert!(error.is_shutdown());
        let source = error.source().unwrap().downcast_ref::<ShutdownError>().unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
        assert!(std::ptr::addr_eq(pool.shutdown().error().unwrap().inner(), expected.inner()));
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
        assert_eq!(size_of::<AsyncTaskInfo>(), 56);
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

        super::add_task_with_metadata::<(), _, _, _, _>(&executor.tasks(), future, arty_executor::TaskPlacement::Any, sender, None, || 0);
        let _ = executor.execute_cycle();

        assert!(ran.get());
    }
}
