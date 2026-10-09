// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use foldhash::{HashMap, HashMapExt};
use nonempty::NonEmpty;
use performables::arc::Arc;
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId, WorkerId};
use seismograph_runtime::task::{TaskHandle, TaskPoll};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};
use seismograph_runtime::{RuntimeHandle, RuntimeMetadata, RuntimeRegistration, register_runtime};

static TYPE_DESCRIPTORS: LazyLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = LazyLock::new(|| Mutex::<_>::new(HashMap::new()));
static NEXT_TYPE_DESCRIPTOR_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static LAST_TYPE_DESCRIPTOR: Cell<Option<(TypeId, TypeDescriptorId)>> = const { Cell::new(None) };
}

#[derive(Debug)]
pub(crate) struct WorkerTelemetry {
    worker_registration: WorkerRegistration,
    _runtime_registration: Arc<RuntimeRegistration>,
    handle: WorkerHandle,
}

impl WorkerTelemetry {
    pub(crate) fn attach_current_thread(&self) {
        self.worker_registration.attach_current_thread();
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeTelemetry {
    runtime_registration: Arc<RuntimeRegistration>,
    runtime_handle: RuntimeHandle,
    worker_handles: NonEmpty<WorkerHandle>,
}

impl RuntimeTelemetry {
    pub(crate) fn register(processor_indices: impl ExactSizeIterator<Item = u32>) -> (Self, Vec<WorkerTelemetry>) {
        let worker_count = processor_indices.len();
        let configured_workers = u32::try_from(worker_count).expect("an Arty runtime cannot configure more than u32::MAX workers");
        let runtime_registration = Arc::new(register_runtime(RuntimeMetadata::new("arty", configured_workers)));
        let runtime_handle = runtime_registration.handle();
        let workers: Vec<_> = processor_indices
            .map(|processor_index| {
                let worker_registration =
                    runtime_registration.register_worker(WorkerMetadata::new(WorkerRole::Core).processor_index(processor_index));
                WorkerTelemetry {
                    handle: worker_registration.handle(),
                    worker_registration,
                    _runtime_registration: Arc::clone(&runtime_registration),
                }
            })
            .collect();
        let worker_handles = NonEmpty::from_vec(workers.iter().map(|worker| worker.handle.clone()).collect())
            .expect("the number of Arty workers is validated before Seismograph registration");

        (
            Self {
                runtime_registration,
                runtime_handle,
                worker_handles,
            },
            workers,
        )
    }

    pub(crate) fn stopping(&self) {
        self.runtime_registration.stopping();
    }

    pub(crate) fn stopped(&self) {
        self.runtime_registration.stopped();
    }

    pub(crate) fn task<T: 'static>(&self, worker_index: usize) -> TaskTelemetryPlacement {
        let worker = self
            .worker_handles
            .get(worker_index)
            .expect("task placement must identify a registered Arty worker")
            .clone();
        let telemetry = TaskTelemetry::spawned::<T>(&self.runtime_handle);
        telemetry.enqueued(Some(worker.id()));
        TaskTelemetryPlacement { telemetry, worker }
    }
}

pub(crate) struct TaskTelemetryPlacement {
    telemetry: TaskTelemetry,
    worker: WorkerHandle,
}

impl TaskTelemetryPlacement {
    pub(crate) fn materialized(mut self) -> TaskTelemetry {
        self.telemetry.materialized(self.worker);
        self.telemetry
    }
}

#[derive(Debug)]
pub(crate) struct TaskTelemetry {
    runtime: RuntimeHandle,
    worker: Option<WorkerHandle>,
    task: Option<TaskHandle>,
}

impl TaskTelemetry {
    fn spawned<T: 'static>(runtime: &RuntimeHandle) -> Self {
        let task = runtime.register_task_with_size(type_descriptor::<T>(), None, size_of::<T>());
        Self {
            runtime: runtime.clone(),
            worker: None,
            task: Some(task),
        }
    }

    fn id(&self) -> TaskId {
        self.task.as_ref().expect("terminal tasks are never instrumented again").id()
    }

    fn enqueued(&self, worker_id: Option<WorkerId>) {
        self.runtime.task_enqueued(self.id(), worker_id);
        self.task.as_ref().expect("new tasks retain their task handle").woken();
    }

    pub(crate) fn materialized(&mut self, worker: WorkerHandle) {
        debug_assert!(self.worker.is_none(), "a task may only be materialized once");
        let worker_id = worker.id();
        self.worker = Some(worker);
        self.runtime.task_materialized(self.id(), worker_id);
    }

    pub(crate) fn poll(&self) -> TaskPollGuard<'_> {
        let worker = self
            .worker
            .as_ref()
            .expect("tasks are materialized before their futures are polled");
        let task = self.task.as_ref().expect("terminal tasks are never polled");
        TaskPollGuard {
            worker,
            task,
            poll: Some(task.poll_started(worker)),
        }
    }

    pub(crate) fn completed(&mut self) {
        if let Some(task_id) = self.begin_terminal() {
            self.runtime.task_completed(task_id, self.worker_id());
        }
    }

    pub(crate) fn panicked(&mut self) {
        if let Some(task_id) = self.begin_terminal() {
            self.runtime.task_panicked(task_id, self.worker_id());
        }
    }

    fn canceled(&mut self) {
        if let Some(task_id) = self.begin_terminal() {
            self.runtime.task_canceled(task_id, self.worker_id());
        }
    }

    fn begin_terminal(&mut self) -> Option<TaskId> {
        self.task.take().map(|task| task.id())
    }

    fn worker_id(&self) -> Option<WorkerId> {
        self.worker.as_ref().map(WorkerHandle::id)
    }
}

impl Drop for TaskTelemetry {
    fn drop(&mut self) {
        self.canceled();
    }
}

pub(crate) struct TaskPollGuard<'a> {
    worker: &'a WorkerHandle,
    task: &'a TaskHandle,
    poll: Option<TaskPoll>,
}

impl Drop for TaskPollGuard<'_> {
    fn drop(&mut self) {
        self.task.poll_finished(
            self.worker,
            self.poll.take().expect("a task poll telemetry guard is finished exactly once"),
        );
    }
}

fn type_descriptor<T: 'static>() -> TypeDescriptorId {
    let type_id = TypeId::of::<T>();
    if let Some(descriptor) = LAST_TYPE_DESCRIPTOR.with(Cell::get).filter(|(cached, _)| *cached == type_id) {
        return descriptor.1;
    }
    let descriptor = *TYPE_DESCRIPTORS
        .lock()
        .expect("the type descriptor cache is never held across user code")
        .entry(type_id)
        .or_insert_with(allocate_type_descriptor_id);
    LAST_TYPE_DESCRIPTOR.with(|cached| cached.set(Some((type_id, descriptor))));
    descriptor
}

fn allocate_type_descriptor_id() -> TypeDescriptorId {
    let raw = NEXT_TYPE_DESCRIPTOR_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
        .expect("a process cannot register u64::MAX task types");
    TypeDescriptorId::from_raw(raw).expect("the type descriptor counter starts at one")
}

#[cfg(test)]
mod tests {
    use super::type_descriptor;

    #[test]
    fn type_descriptor_cache_is_stable_and_type_specific() {
        assert_eq!(type_descriptor::<u8>(), type_descriptor::<u8>());
        assert_ne!(type_descriptor::<u8>(), type_descriptor::<u16>());
    }
}
