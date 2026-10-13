// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use foldhash::{HashMap, HashMapExt};
use nonempty::NonEmpty;
use observed::{Sink, emit};
use performables::arc::Arc;
use seismograph::recorder::SuppressionGuard;
use seismograph::recorder::event::{EventClass, EventTimestamp};
use seismograph::recorder::runtime::{RuntimeId as SeismographRuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph_runtime::snapshot::{Task, TaskActivity, TaskActivityState, TaskMetrics, TaskMetricsScope};
use seismograph_runtime::task::{LiveTask, PendingTask};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};
use seismograph_runtime::{RuntimeHandle, RuntimeMetadata, RuntimeRegistration, allocate_type_descriptor_id, register_runtime};

use crate::runtime::telemetry::events::RuntimeStopped;
use crate::task::execution::instrumentation::{PendingInstrumentation, TaskInstrumentation};

static TYPE_DESCRIPTORS: LazyLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = LazyLock::new(|| Mutex::<_>::new(HashMap::new()));

const TYPE_DESCRIPTOR_CACHE_SIZE: usize = 4;

thread_local! {
    static LAST_TYPE_DESCRIPTOR: Cell<Option<(TypeId, TypeDescriptorId)>> = const { Cell::new(None) };
    static RECENT_TYPE_DESCRIPTORS: Cell<[Option<(TypeId, TypeDescriptorId)>; TYPE_DESCRIPTOR_CACHE_SIZE]> =
        const { Cell::new([None; TYPE_DESCRIPTOR_CACHE_SIZE]) };
    static ACTIVE_TASK: Cell<Option<ActiveTask>> = const { Cell::new(None) };
    #[cfg(test)]
    static TYPE_DESCRIPTOR_CACHE_MISSES: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn recording_requested() -> bool {
    seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask)
}

pub(crate) enum TaskDescription {
    Identity(TaskId),
    Snapshot(Box<Task>),
}

pub(crate) fn inspection_identity(identity: &Cell<Option<TaskId>>) -> TaskId {
    identity.get().unwrap_or_else(|| {
        let id = seismograph_runtime::allocate_task_id();
        identity.set(Some(id));
        id
    })
}

pub(crate) fn describe_task<F: 'static>(identity: &Cell<Option<TaskId>>, state: arty_executor::TaskState) -> TaskDescription {
    let id = inspection_identity(identity);
    TaskDescription::Snapshot(Box::new(Task {
        id,
        parent: None,
        type_descriptor: type_descriptor_id::<F>(),
        future_size_bytes: Some(size_of::<F>() as u64),
        spawned_at: EventTimestamp::from_ticks(0),
        last_worker_id: None,
        metrics: TaskMetrics::default(),
        metrics_scope: TaskMetricsScope::Uncollected,
        activity: Some(TaskActivity {
            observed_at: EventTimestamp::now(),
            state: match state {
                arty_executor::TaskState::Ready => TaskActivityState::Ready,
                arty_executor::TaskState::Waiting => TaskActivityState::Waiting,
                arty_executor::TaskState::Running => TaskActivityState::Unknown,
            },
            ready_since: None,
            poll_started_at: None,
            poll_worker_id: None,
            queued_since: None,
        }),
        spawn_backtrace: Vec::new(),
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservationWindow {
    Live(u64),
    Recording(seismograph::recorder::RecordingSession),
}

#[derive(Debug)]
struct MeasuredTask {
    window: ObservationWindow,
    since: EventTimestamp,
    metrics: TaskMetrics,
}

#[derive(Debug)]
struct ObservedPoll {
    task: TaskId,
    window: Option<ObservationWindow>,
    poll: Option<seismograph_runtime::task::TaskPoll>,
    previous_task: Option<ActiveTask>,
    tracks_parent: bool,
}

/// This belongs to the worker once; disconnected tasks never own diagnostic handles.
#[derive(Debug)]
pub(crate) struct WorkerObservation {
    worker: WorkerHandle,
    measured: RefCell<Option<HashMap<TaskId, MeasuredTask>>>,
    active: RefCell<Option<ObservedPoll>>,
    failed: Cell<bool>,
}

impl WorkerObservation {
    pub(crate) fn new(worker: WorkerHandle) -> Self {
        Self {
            worker,
            measured: RefCell::new(None),
            active: RefCell::new(None),
            failed: Cell::new(false),
        }
    }

    fn window(&self) -> Option<ObservationWindow> {
        let generation = self.worker.live_timing_generation();
        if generation != 0 {
            Some(ObservationWindow::Live(generation))
        } else {
            seismograph::recorder::active_recording_session().map(ObservationWindow::Recording)
        }
    }

    pub(crate) fn describe(&self, description: &TaskDescription) -> Task {
        let TaskDescription::Snapshot(task) = description else {
            unreachable!("native queue inspection requests snapshots, not poll identities");
        };
        let mut task = task.as_ref().clone();
        task.last_worker_id = Some(self.worker.id());
        task.metrics = TaskMetrics::default();
        task.metrics_scope = TaskMetricsScope::Uncollected;
        let measured = self.measured.borrow();
        if let Some(measurement) = measured
            .as_ref()
            .and_then(|tasks| tasks.get(&task.id))
            .filter(|measurement| Some(measurement.window) == self.window())
        {
            task.metrics = measurement.metrics;
            task.metrics_scope = TaskMetricsScope::PollsSince(measurement.since);
        }

        task
    }

    pub(crate) fn take_failure(&self) -> bool {
        self.failed.replace(false)
    }
}

impl arty_executor::PollObserver for WorkerObservation {
    fn inspection_failed(&self, _error: arty_executor::InspectionError) {
        if let Some(active) = self.active.borrow_mut().take()
            && active.tracks_parent
        {
            ACTIVE_TASK.set(active.previous_task);
        }
        self.worker.task_left();
        self.failed.set(true);
    }

    fn enabled(&self) -> bool {
        let recording = recording_requested();
        let enabled = self.worker.live_observation_enabled() || recording;
        if self.worker.live_timing_generation() == 0 && !recording {
            self.measured.borrow_mut().take();
        }
        enabled
    }

    fn poll_started(&self, description: &dyn std::any::Any) {
        let Some(description) = description.downcast_ref::<TaskDescription>() else {
            return;
        };
        let TaskDescription::Identity(id) = *description else {
            unreachable!("poll observers request task identities, not snapshots");
        };
        let tracks_parent = recording_requested();
        let window = if self.worker.live_timing_generation() != 0 || tracks_parent {
            self.window()
        } else {
            None
        };
        let poll = if window.is_some() {
            Some(self.worker.task_poll_started(id))
        } else {
            self.worker.task_entered(id);
            None
        };
        let previous_task = if tracks_parent {
            ACTIVE_TASK.replace(Some(ActiveTask {
                runtime_id: self.worker.runtime_id(),
                task_id: id,
            }))
        } else {
            None
        };
        *self.active.borrow_mut() = Some(ObservedPoll {
            task: id,
            window,
            poll,
            previous_task,
            tracks_parent,
        });
    }

    fn poll_finished(&self, completed: bool) {
        let Some(active) = self.active.borrow_mut().take() else {
            return;
        };
        if active.tracks_parent {
            ACTIVE_TASK.set(active.previous_task);
        }
        if let Some(poll) = active.poll {
            let started_at = poll.started_at();
            let elapsed = self.worker.task_poll_finished_measured(poll);
            if active.window == self.window() {
                let mut measured = self.measured.borrow_mut();
                let tasks = measured.get_or_insert_with(HashMap::new);
                let window = active.window.expect("measured polls retain their observation window");
                let measurement = tasks.entry(active.task).or_insert_with(|| MeasuredTask {
                    window,
                    since: started_at,
                    metrics: TaskMetrics::default(),
                });
                if measurement.window != window {
                    *measurement = MeasuredTask {
                        window,
                        since: started_at,
                        metrics: TaskMetrics::default(),
                    };
                }
                measurement.metrics.poll_count = measurement.metrics.poll_count.saturating_add(1);
                measurement.metrics.poll_duration_nanos = measurement.metrics.poll_duration_nanos.saturating_add(elapsed);
                measurement.metrics.max_poll_duration_nanos = measurement.metrics.max_poll_duration_nanos.max(elapsed);
            }
        } else {
            self.worker.task_left();
        }
        if completed && let Some(tasks) = self.measured.borrow_mut().as_mut() {
            tasks.remove(&active.task);
        }
    }
}

#[derive(Clone, Copy, Debug)]
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
    observed_stopped_reported: AtomicBool,
    sink: Sink,
}

impl RuntimeLifecycle {
    fn worker_stopped(&self) {
        let previous = self.workers_remaining.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "each runtime worker retires exactly once");
        if previous == 1 {
            self.registration.stopped();
        }
    }

    fn stopped(&self) {
        self.registration.stopped();
        if !self.observed_stopped_reported.swap(true, Ordering::Relaxed) {
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
    pub(crate) fn register_task_provider(
        &self,
        worker_index: usize,
        provider: impl Fn(seismograph_runtime::live::TaskRequest) + Send + Sync + 'static,
    ) {
        self.worker_handles
            .get(worker_index)
            .expect("live providers identify registered workers")
            .register_task_provider(provider)
            .expect("bootstrap registers task providers before worker retirement");
    }

    pub(crate) fn register(
        processor_indices: impl ExactSizeIterator<Item = u32>,
        sink: Sink,
    ) -> (Self, std::vec::IntoIter<WorkerTelemetry>) {
        let worker_count = processor_indices.len();
        let configured_workers = u32::try_from(worker_count).expect("an Arty runtime cannot configure more than u32::MAX workers");
        let registration = register_runtime(RuntimeMetadata::new("arty", configured_workers));
        let runtime_handle = registration.handle();
        let lifecycle = Arc::new(RuntimeLifecycle {
            registration,
            workers_remaining: AtomicUsize::new(worker_count),
            observed_stopped_reported: AtomicBool::new(false),
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
            workers.into_iter(),
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
        let parent = ACTIVE_TASK
            .get()
            .filter(|active| active.runtime_id == self.runtime_handle.id())
            .map(|active| active.task_id);
        TaskTelemetryPlacement {
            pending: Some(worker.prepare_live_task(
                descriptor.type_descriptor.unwrap_or_else(allocate_type_descriptor_id),
                parent,
                descriptor.future_size_bytes,
            )),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TaskDescriptor {
    type_descriptor: Option<TypeDescriptorId>,
    future_size_bytes: usize,
}

impl TaskDescriptor {
    pub(crate) fn future_size_bytes(self) -> usize {
        self.future_size_bytes
    }

    pub(crate) fn of<T: 'static>() -> Self {
        Self {
            type_descriptor: Some(type_descriptor_id::<T>()),
            future_size_bytes: size_of::<T>(),
        }
    }

    pub(crate) fn of_scoped<T>() -> Self {
        Self {
            type_descriptor: None,
            future_size_bytes: size_of::<T>(),
        }
    }
}

pub(crate) struct TaskTelemetryPlacement {
    pending: Option<PendingTask>,
}

impl TaskTelemetryPlacement {
    pub(crate) fn enqueued(&self) {
        self.pending
            .as_ref()
            .expect("pending instrumentation is transferred only on materialization")
            .enqueued();
    }

    pub(crate) fn materialized(mut self) -> TaskTelemetry {
        let pending = self.pending.take().expect("task instrumentation materializes exactly once");
        if !recording_requested() {
            pending.stop_observing();
            return TaskTelemetry { worker: None, task: None };
        }
        let (task, worker) = pending.materialized();
        TaskTelemetry {
            worker: Some(worker),
            task: Some(task),
        }
    }

    pub(crate) fn panicked(mut self) {
        if let Some(pending) = self.pending.take() {
            if recording_requested() {
                pending.panicked();
            } else {
                pending.stop_observing();
            }
        }
    }
}

impl Drop for TaskTelemetryPlacement {
    fn drop(&mut self) {
        if !recording_requested()
            && let Some(pending) = self.pending.take()
        {
            pending.stop_observing();
        }
    }
}

impl PendingInstrumentation for TaskTelemetryPlacement {
    type Task = TaskTelemetry;
    fn enqueued(&self) {
        self.enqueued();
    }
    fn materialized(self) -> Self::Task {
        self.materialized()
    }
    fn panicked(self) {
        self.panicked();
    }
}

#[derive(Debug)]
pub(crate) struct TaskTelemetry {
    worker: Option<WorkerHandle>,
    task: Option<LiveTask>,
}

impl TaskTelemetry {
    pub(crate) fn completed(&mut self) {
        if let Some(mut task) = self.begin_terminal() {
            if recording_requested() {
                task.completed(self.worker_id());
            } else {
                task.stop_observing();
            }
        }
    }

    pub(crate) fn panicked(&mut self) {
        if let Some(mut task) = self.begin_terminal() {
            if recording_requested() {
                task.panicked(self.worker_id());
            } else {
                task.stop_observing();
            }
        }
    }

    pub(crate) fn canceled(&mut self) {
        if let Some(mut task) = self.begin_terminal() {
            if recording_requested() {
                task.canceled(self.worker_id());
            } else {
                task.stop_observing();
            }
        }
    }

    fn begin_terminal(&mut self) -> Option<LiveTask> {
        self.task.take()
    }

    fn worker_id(&self) -> Option<WorkerId> {
        self.worker.as_ref().map(WorkerHandle::id)
    }

    pub(crate) fn snapshot(&self, state: arty_executor::TaskState) -> Option<seismograph_runtime::snapshot::Task> {
        let state = match state {
            arty_executor::TaskState::Ready => seismograph_runtime::snapshot::TaskActivityState::Ready,
            arty_executor::TaskState::Waiting => seismograph_runtime::snapshot::TaskActivityState::Waiting,
            arty_executor::TaskState::Running => seismograph_runtime::snapshot::TaskActivityState::Unknown,
        };
        self.task.as_ref().map(|task| task.snapshot(state, self.worker_id()))
    }
}

impl TaskInstrumentation for TaskTelemetry {
    fn identity(&self) -> Option<TaskId> {
        self.task.as_ref().map(LiveTask::id)
    }
    fn completed(&mut self) {
        self.completed();
    }
    fn canceled(&mut self) {
        self.canceled();
    }
    fn panicked(&mut self) {
        self.panicked();
    }
    fn snapshot(&self, state: arty_executor::TaskState) -> Option<Task> {
        self.snapshot(state)
    }
}

impl Drop for TaskTelemetry {
    fn drop(&mut self) {
        self.canceled();
    }
}

fn type_descriptor_id<T: 'static>() -> TypeDescriptorId {
    let type_id = TypeId::of::<T>();
    if let Some((_, descriptor)) = LAST_TYPE_DESCRIPTOR.get().filter(|(cached, _)| *cached == type_id) {
        return descriptor;
    }
    let cache = RECENT_TYPE_DESCRIPTORS.get();
    if let Some((_, descriptor)) = cache.into_iter().flatten().find(|(cached, _)| *cached == type_id) {
        LAST_TYPE_DESCRIPTOR.set(Some((type_id, descriptor)));
        return descriptor;
    }
    let _suppression = SuppressionGuard::enter();
    let descriptor = *TYPE_DESCRIPTORS
        .lock()
        .expect("the type descriptor cache is never held across user code")
        .entry(type_id)
        .or_insert_with(allocate_type_descriptor_id);
    RECENT_TYPE_DESCRIPTORS.set([Some((type_id, descriptor)), cache[0], cache[1], cache[2]]);
    LAST_TYPE_DESCRIPTOR.set(Some((type_id, descriptor)));
    #[cfg(test)]
    TYPE_DESCRIPTOR_CACHE_MISSES.set(TYPE_DESCRIPTOR_CACHE_MISSES.get().saturating_add(1));
    descriptor
}

#[cfg(test)]
mod tests {
    use observed_testing::{TEST_ID, test_emitter};
    use seismograph_runtime::allocate_type_descriptor_id;
    use seismograph_runtime::snapshot::{RuntimeState, source};

    use super::{TYPE_DESCRIPTOR_CACHE_MISSES, TaskDescriptor, type_descriptor_id};

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
    fn heterogeneous_type_descriptor_cache_stays_thread_local_after_warmup() {
        let _ = type_descriptor_id::<u32>();
        let _ = type_descriptor_id::<u64>();
        let _ = type_descriptor_id::<i32>();
        let _ = type_descriptor_id::<i64>();
        let misses = TYPE_DESCRIPTOR_CACHE_MISSES.get();

        for _ in 0..100 {
            let _ = type_descriptor_id::<u32>();
            let _ = type_descriptor_id::<u64>();
            let _ = type_descriptor_id::<i32>();
            let _ = type_descriptor_id::<i64>();
        }

        assert_eq!(TYPE_DESCRIPTOR_CACHE_MISSES.get(), misses);
    }

    #[test]
    fn unrecorded_scoped_descriptors_defer_identity_and_retain_future_sizes() {
        let first = TaskDescriptor::of_scoped::<u8>();
        let second = TaskDescriptor::of_scoped::<u8>();
        let other = TaskDescriptor::of_scoped::<u16>();
        assert_eq!(
            (first.type_descriptor, second.type_descriptor, other.type_descriptor),
            (None, None, None)
        );
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

    #[test]
    fn final_worker_retirement_stops_the_runtime() {
        let (sink, processor) = test_emitter(TEST_ID);
        let (runtime, workers) = super::RuntimeTelemetry::register(0..1, sink);
        let runtime_id = runtime.id();

        drop(workers);

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
            0
        );
    }

    #[test]
    fn a_failed_poll_description_is_reported_and_discards_partial_observation_state() {
        use arty_executor::PollObserver as _;

        let (sink, _processor) = test_emitter(TEST_ID);
        let (runtime, workers) = super::RuntimeTelemetry::register(0..1, sink);
        let observer = super::WorkerObservation::new(runtime.worker_handles[0].clone());
        let task_id = seismograph_runtime::allocate_task_id();
        observer.poll_started(&super::TaskDescription::Identity(task_id));
        assert!(observer.active.borrow().is_some());
        observer.inspection_failed(arty_executor::InspectionError::Panicked);
        assert!(observer.active.borrow().is_none());
        assert!(observer.take_failure());
        assert!(!observer.take_failure());
        drop(workers);
    }
}
