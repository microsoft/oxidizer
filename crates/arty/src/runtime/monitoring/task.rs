// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

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

pub(crate) fn current_task() -> Option<TaskId> {
    arty_executor::current_task().or_else(|| CURRENT.with(Cell::get))
}

#[derive(Debug)]
#[must_use]
pub(crate) struct CurrentTask(Option<TaskId>);

impl Drop for CurrentTask {
    fn drop(&mut self) {
        CURRENT.with(|slot| slot.set(self.0));
    }
}

pub(crate) fn enter_current(task: TaskId) -> CurrentTask {
    CurrentTask(CURRENT.with(|slot| slot.replace(Some(task))))
}

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::{TaskInfo, current_task, enter_current};

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
    fn unobserved_blocking_outcomes_do_not_allocate_task_identities() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("idle-outcomes", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Blocking));
        seismograph::recorder::without_recording(|| {
            let info = TaskInfo::new::<()>(worker.handle().task_events());
            info.completed();
            info.panicked();
            info.canceled();
            assert!(info.identity.get().is_none());
        });
    }
}
