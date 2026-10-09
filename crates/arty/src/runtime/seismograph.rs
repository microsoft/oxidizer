// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use nonempty::NonEmpty;
use performables::arc::Arc;
use performables::sync::mutex::Mutex;
use performables::sync::once::LazyLock;
use seismograph::recorder::runtime::{TaskId, TypeDescriptorId, WorkerId};
use seismograph_runtime::task::{TaskHandle, TaskPoll};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};
use seismograph_runtime::{RuntimeHandle, RuntimeMetadata, RuntimeRegistration, register_runtime};

static TYPE_DESCRIPTORS: LazyLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = LazyLock::new(|| Mutex::<_>::new(HashMap::new()));
static NEXT_TYPE_DESCRIPTOR_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(crate) struct WorkerTelemetry {
    worker_registration: Arc<WorkerRegistration>,
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
    worker_registrations: Mutex<Option<NonEmpty<WorkerTelemetry>>>,
    runtime_registration: Arc<RuntimeRegistration>,
    runtime_handle: RuntimeHandle,
    worker_handles: NonEmpty<WorkerHandle>,
}

impl RuntimeTelemetry {
    pub(crate) fn register(worker_count: usize) -> (Self, Vec<WorkerTelemetry>) {
        let configured_workers = u32::try_from(worker_count).expect("an Arty runtime cannot configure more than u32::MAX workers");
        let runtime_registration = Arc::new(register_runtime(RuntimeMetadata::new("arty", configured_workers)));
        let runtime_handle = runtime_registration.handle();
        let workers: Vec<_> = (0..worker_count)
            .map(|worker_index| {
                let processor_index = u32::try_from(worker_index).expect("an Arty runtime cannot configure more than u32::MAX workers");
                let worker_registration =
                    Arc::new(runtime_registration.register_worker(WorkerMetadata::new(WorkerRole::Core).processor_index(processor_index)));
                WorkerTelemetry {
                    handle: worker_registration.handle(),
                    worker_registration,
                    _runtime_registration: Arc::clone(&runtime_registration),
                }
            })
            .collect();
        let worker_registrations =
            NonEmpty::from_vec(workers.clone()).expect("the number of Arty workers is validated before Seismograph registration");
        let worker_handles = NonEmpty::from_vec(workers.iter().map(|worker| worker.handle.clone()).collect())
            .expect("the number of Arty workers is validated before Seismograph registration");

        (
            Self {
                worker_registrations: Mutex::new(Some(worker_registrations)),
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
        drop(self.worker_registrations.lock().take());
        self.runtime_registration.stopped();
    }

    pub(crate) fn task<T: 'static>(&self, worker_index: usize) -> (TaskTelemetry, WorkerHandle) {
        let worker = self
            .worker_handles
            .get(worker_index)
            .expect("task placement must identify a registered Arty worker")
            .clone();
        let telemetry = TaskTelemetry::spawned::<T>(&self.runtime_handle);
        telemetry.enqueued(Some(worker.id()));
        (telemetry, worker)
    }
}

#[derive(Debug)]
pub(crate) struct TaskTelemetry {
    runtime: RuntimeHandle,
    worker: Option<WorkerHandle>,
    task: TaskHandle,
    terminal: bool,
    materialized: bool,
}

impl TaskTelemetry {
    fn spawned<T: 'static>(runtime: &RuntimeHandle) -> Self {
        let task = runtime.register_task_with_size(type_descriptor::<T>(), None, size_of::<T>());
        Self {
            runtime: runtime.clone(),
            worker: None,
            task,
            terminal: false,
            materialized: false,
        }
    }

    fn id(&self) -> TaskId {
        self.task.id()
    }

    fn enqueued(&self, worker_id: Option<WorkerId>) {
        self.runtime.task_enqueued(self.id(), worker_id);
        self.task.woken();
    }

    pub(crate) fn materialized(&mut self, worker: &WorkerHandle) {
        debug_assert!(!self.materialized, "a task may only be materialized once");
        self.worker = Some(worker.clone());
        self.materialized = true;
        self.runtime.task_materialized(self.id(), worker.id());
    }

    pub(crate) fn poll(&self) -> TaskPollGuard<'_> {
        let worker = self
            .worker
            .as_ref()
            .expect("tasks are materialized before their futures are polled");
        TaskPollGuard {
            worker,
            task: &self.task,
            poll: Some(self.task.poll_started(worker)),
        }
    }

    pub(crate) fn completed(&mut self) {
        if self.begin_terminal() {
            self.runtime.task_completed(self.id(), self.worker_id());
        }
    }

    pub(crate) fn panicked(&mut self) {
        if self.begin_terminal() {
            self.runtime.task_panicked(self.id(), self.worker_id());
        }
    }

    fn canceled(&mut self) {
        if self.begin_terminal() {
            self.runtime.task_canceled(self.id(), self.worker_id());
        }
    }

    fn begin_terminal(&mut self) -> bool {
        if self.terminal {
            false
        } else {
            self.terminal = true;
            true
        }
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
    *TYPE_DESCRIPTORS
        .lock()
        .entry(TypeId::of::<T>())
        .or_insert_with(allocate_type_descriptor_id)
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
