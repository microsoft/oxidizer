// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use foldhash::{HashMap, HashMapExt};
use nonempty::NonEmpty;
use observed::{Sink, emit};
use performables::arc::Arc;
use seismograph::recorder::SuppressionGuard;
use seismograph::recorder::runtime::{RuntimeId as SeismographRuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph_runtime::task::{TaskHandle, TaskPoll};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};
use seismograph_runtime::{RuntimeHandle, RuntimeMetadata, RuntimeRegistration, allocate_type_descriptor_id, register_runtime};

use crate::runtime::telemetry::events::RuntimeStopped;

static TYPE_DESCRIPTORS: LazyLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = LazyLock::new(|| Mutex::<_>::new(HashMap::new()));

thread_local! {
    static LAST_TYPE_DESCRIPTOR: Cell<Option<(TypeId, TypeDescriptorId)>> = const { Cell::new(None) };
    static ACTIVE_TASK: Cell<Option<ActiveTask>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct ActiveTask {
    runtime_id: SeismographRuntimeId,
    task_id: TaskId,
}

#[derive(Debug)]
pub(crate) struct WorkerTelemetry {
    worker_registration: Option<WorkerRegistration>,
    lifecycle: Arc<RuntimeLifecycle>,
    handle: WorkerHandle,
}

impl WorkerTelemetry {
    pub(crate) fn attach_current_thread(&self) {
        self.worker_registration
            .as_ref()
            .expect("a live worker retains its registration")
            .attach_current_thread();
    }

    pub(crate) fn handle(&self) -> WorkerHandle {
        self.handle.clone()
    }
}

impl Drop for WorkerTelemetry {
    fn drop(&mut self) {
        drop(self.worker_registration.take());
        self.lifecycle.worker_stopped();
    }
}

#[derive(Debug)]
struct RuntimeLifecycle {
    registration: RuntimeRegistration,
    workers_remaining: AtomicUsize,
    stopped_reported: AtomicBool,
    sink: Sink,
}

impl RuntimeLifecycle {
    fn worker_stopped(&self) {
        let previous = self.workers_remaining.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "each runtime worker retires exactly once");
        if previous == 1 {
            self.stopped();
        }
    }

    fn stopped(&self) {
        self.registration.stopped();
        if !self.stopped_reported.swap(true, Ordering::Relaxed) {
            emit!(&self.sink, RuntimeStopped);
        }
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeTelemetry {
    lifecycle: Arc<RuntimeLifecycle>,
    runtime_handle: RuntimeHandle,
    worker_handles: NonEmpty<WorkerHandle>,
}

impl RuntimeTelemetry {
    pub(crate) fn register(processor_indices: impl ExactSizeIterator<Item = u32>, sink: Sink) -> (Self, Vec<WorkerTelemetry>) {
        let worker_count = processor_indices.len();
        let configured_workers = u32::try_from(worker_count).expect("an Arty runtime cannot configure more than u32::MAX workers");
        let registration = register_runtime(RuntimeMetadata::new("arty", configured_workers));
        let runtime_handle = registration.handle();
        let lifecycle = Arc::new(RuntimeLifecycle {
            registration,
            workers_remaining: AtomicUsize::new(worker_count),
            stopped_reported: AtomicBool::new(false),
            sink,
        });
        let workers: Vec<_> = processor_indices
            .map(|processor_index| {
                let worker_registration = lifecycle
                    .registration
                    .register_worker(WorkerMetadata::new(WorkerRole::Core).processor_index(processor_index));
                WorkerTelemetry {
                    handle: worker_registration.handle(),
                    worker_registration: Some(worker_registration),
                    lifecycle: Arc::clone(&lifecycle),
                }
            })
            .collect();
        let worker_handles = NonEmpty::from_vec(workers.iter().map(|worker| worker.handle.clone()).collect())
            .expect("the number of Arty workers is validated before Seismograph registration");

        (
            Self {
                lifecycle,
                runtime_handle,
                worker_handles,
            },
            workers,
        )
    }

    pub(crate) fn stopping(&self) {
        self.lifecycle.registration.stopping();
    }

    pub(crate) fn stopped(&self) {
        self.lifecycle.stopped();
    }

    pub(crate) fn id(&self) -> SeismographRuntimeId {
        self.lifecycle.registration.id()
    }

    pub(crate) fn register_task<T: 'static>(&self, worker_index: usize) -> TaskTelemetryPlacement {
        self.register_task_with_descriptor(worker_index, TaskDescriptor::of::<T>())
    }

    pub(crate) fn register_task_with_descriptor(&self, worker_index: usize, descriptor: TaskDescriptor) -> TaskTelemetryPlacement {
        let worker = self
            .worker_handles
            .get(worker_index)
            .expect("task placement must identify a registered Arty worker")
            .clone();
        let telemetry = TaskTelemetry::spawned(&self.runtime_handle, descriptor);
        TaskTelemetryPlacement { telemetry, worker }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TaskDescriptor {
    type_descriptor: TypeDescriptorId,
    future_size_bytes: usize,
}

impl TaskDescriptor {
    pub(crate) fn of<T: 'static>() -> Self {
        Self {
            type_descriptor: type_descriptor_id::<T>(),
            future_size_bytes: size_of::<T>(),
        }
    }

    pub(crate) fn of_scoped<T>() -> Self {
        let _suppression = SuppressionGuard::enter();
        Self {
            type_descriptor: allocate_type_descriptor_id(),
            future_size_bytes: size_of::<T>(),
        }
    }
}

pub(crate) struct TaskTelemetryPlacement {
    telemetry: TaskTelemetry,
    worker: WorkerHandle,
}

impl TaskTelemetryPlacement {
    pub(crate) fn enqueued(&self) {
        self.telemetry.enqueued(self.worker.id());
    }

    pub(crate) fn materialized(mut self) -> TaskTelemetry {
        self.telemetry.materialized(self.worker);
        self.telemetry
    }

    pub(crate) fn panicked(mut self) {
        self.telemetry.panicked();
    }
}

#[derive(Debug)]
pub(crate) struct TaskTelemetry {
    runtime: RuntimeHandle,
    worker: Option<WorkerHandle>,
    task: Option<TaskHandle>,
}

impl TaskTelemetry {
    fn spawned(runtime: &RuntimeHandle, descriptor: TaskDescriptor) -> Self {
        let parent = ACTIVE_TASK
            .get()
            .filter(|active| active.runtime_id == runtime.id())
            .map(|active| active.task_id);
        let task = runtime.register_task_with_size(descriptor.type_descriptor, parent, descriptor.future_size_bytes);
        Self {
            runtime: runtime.clone(),
            worker: None,
            task: Some(task),
        }
    }

    fn id(&self) -> TaskId {
        self.task.as_ref().expect("terminal tasks are never instrumented again").id()
    }

    fn enqueued(&self, worker_id: WorkerId) {
        self.runtime.task_enqueued(self.id(), Some(worker_id));
        self.task.as_ref().expect("new tasks retain their task handle").woken();
    }

    fn materialized(&mut self, worker: WorkerHandle) {
        debug_assert!(self.worker.is_none(), "a task may only be materialized once");
        let worker_id = worker.id();
        self.worker = Some(worker);
        self.runtime.task_materialized(self.id(), worker_id);
    }

    pub(crate) fn poll_started(&self) -> TaskPollGuard<'_> {
        let worker = self
            .worker
            .as_ref()
            .expect("tasks are materialized before their futures are polled");
        let task = self.task.as_ref().expect("terminal tasks are never polled");
        let poll = task.poll_started(worker);
        let previous_task = ACTIVE_TASK.replace(Some(ActiveTask {
            runtime_id: self.runtime.id(),
            task_id: task.id(),
        }));
        TaskPollGuard {
            worker,
            task,
            poll: Some(poll),
            previous_task,
        }
    }

    pub(crate) fn completed(&mut self) {
        if let Some(task) = self.begin_terminal() {
            task.completed(self.worker_id());
        }
    }

    pub(crate) fn panicked(&mut self) {
        if let Some(task) = self.begin_terminal() {
            task.panicked(self.worker_id());
        }
    }

    pub(crate) fn canceled(&mut self) {
        if let Some(task) = self.begin_terminal() {
            task.canceled(self.worker_id());
        }
    }

    fn begin_terminal(&mut self) -> Option<TaskHandle> {
        self.task.take()
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
    previous_task: Option<ActiveTask>,
}

impl Drop for TaskPollGuard<'_> {
    fn drop(&mut self) {
        ACTIVE_TASK.set(self.previous_task);
        self.task.poll_finished(
            self.worker,
            self.poll.take().expect("a task poll telemetry guard is finished exactly once"),
        );
    }
}

fn type_descriptor_id<T: 'static>() -> TypeDescriptorId {
    let type_id = TypeId::of::<T>();
    if let Some(descriptor) = LAST_TYPE_DESCRIPTOR.with(Cell::get).filter(|(cached, _)| *cached == type_id) {
        return descriptor.1;
    }
    let _suppression = SuppressionGuard::enter();
    let descriptor = *TYPE_DESCRIPTORS
        .lock()
        .expect("the type descriptor cache is never held across user code")
        .entry(type_id)
        .or_insert_with(allocate_type_descriptor_id);
    LAST_TYPE_DESCRIPTOR.with(|cached| cached.set(Some((type_id, descriptor))));
    descriptor
}

#[cfg(test)]
mod tests {
    use observed_testing::{TEST_ID, test_emitter};
    use seismograph_runtime::allocate_type_descriptor_id;
    use seismograph_runtime::snapshot::{RuntimeState, source};

    use super::{TaskDescriptor, type_descriptor_id};

    #[test]
    fn type_descriptor_cache_is_stable_and_type_specific() {
        struct ArtyDescriptor;

        let before = allocate_type_descriptor_id();
        let arty = type_descriptor_id::<ArtyDescriptor>();
        let after = allocate_type_descriptor_id();
        assert_eq!(type_descriptor_id::<u8>(), type_descriptor_id::<u8>());
        assert_ne!(type_descriptor_id::<u8>(), type_descriptor_id::<u16>());
        assert!(before.get() < arty.get());
        assert!(arty.get() < after.get());
    }

    #[test]
    fn scoped_type_descriptors_are_unique_and_retain_future_sizes() {
        let first = TaskDescriptor::of_scoped::<u8>();
        let second = TaskDescriptor::of_scoped::<u8>();
        let other = TaskDescriptor::of_scoped::<u16>();
        assert_ne!(first.type_descriptor, second.type_descriptor);
        assert_ne!(second.type_descriptor, other.type_descriptor);
        assert_eq!(
            (first.future_size_bytes, second.future_size_bytes, other.future_size_bytes),
            (size_of::<u8>(), size_of::<u8>(), size_of::<u16>())
        );
    }

    #[test]
    fn runtime_stopped_is_reported_once_across_join_and_worker_retirement() {
        let (sink, processor) = test_emitter(TEST_ID);
        let (runtime, workers) = super::RuntimeTelemetry::register(0..1, sink);
        let runtime_id = runtime.id();
        runtime.stopped();
        runtime.stopped();

        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let source = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
        let runtime_snapshot = seismograph_runtime::snapshot::decode(&source.data).unwrap();
        assert_eq!(
            runtime_snapshot
                .runtimes
                .iter()
                .find(|runtime| runtime.id == runtime_id)
                .unwrap()
                .state,
            RuntimeState::Stopped
        );
        assert_eq!(
            processor.events().iter().filter(|event| event.name() == "arty.rt.stopped").count(),
            1
        );
        drop(workers);
        assert_eq!(
            processor.events().iter().filter(|event| event.name() == "arty.rt.stopped").count(),
            1
        );
    }
}
