// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task lifecycle instrumentation for logical runtimes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::RecordingSession;
use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp};
use seismograph::recorder::runtime::{RuntimeId, TaskId, TypeDescriptorId, WorkerId};

use crate::worker::WorkerHandle;
use crate::{RuntimeHandle, TaskControl, TaskOutcome, duration_nanos};

/// Cheap task handle used to correlate wake notifications with subsequent polls.
#[derive(Clone, Debug)]
pub struct TaskHandle {
    pub(crate) task: Arc<TaskControl>,
}

impl TaskHandle {
    pub(crate) fn new(task: Arc<TaskControl>) -> Self {
        Self { task }
    }

    /// Returns this task's process-monotonic identity.
    #[must_use]
    pub fn id(&self) -> TaskId {
        self.task.id
    }

    /// Reports successful task completion and retires this handle's live task.
    ///
    /// The first terminal report wins. Reporting through another cloned handle
    /// or the runtime's compatibility APIs after retirement has no effect.
    pub fn completed(self, worker: Option<WorkerId>) {
        self.finish(worker, TaskOutcome::Completed);
    }

    /// Reports task cancellation and retires this handle's live task.
    ///
    /// Dropping a task handle does not imply cancellation; the task owner must
    /// call this method when it abandons the task.
    pub fn canceled(self, worker: Option<WorkerId>) {
        self.finish(worker, TaskOutcome::Canceled);
    }

    /// Reports a task panic and retires this handle's live task.
    ///
    /// The first terminal report wins. Later reports through outstanding clones
    /// are ignored.
    pub fn panicked(self, worker: Option<WorkerId>) {
        self.finish(worker, TaskOutcome::Panicked);
    }

    fn finish(self, worker: Option<WorkerId>, outcome: TaskOutcome) {
        let control = self
            .task
            .runtime
            .upgrade()
            .expect("the process registry retains every registered runtime control");
        RuntimeHandle { control }.complete_task_handle(self.task, worker, outcome);
    }

    /// Marks the task ready to run, retaining only the first wake before its next poll.
    #[inline]
    pub fn woken(&self) {
        if seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask) {
            self.task.activity.woken(&self.task);
        } else {
            self.task.wake_timestamp();
        }
    }

    /// Starts a poll and records how long the task waited after becoming ready.
    #[inline]
    pub fn poll_started(&self, worker: &WorkerHandle) -> TaskPoll {
        if seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask) {
            return self.poll_started_recorded(worker);
        }
        let (started_at, ready_since) = self.task.poll_timestamps();
        self.update_poll_metrics(worker, started_at, ready_since);
        worker.task_poll_started_at(self.id(), started_at, None)
    }

    #[cold]
    fn poll_started_recorded(&self, worker: &WorkerHandle) -> TaskPoll {
        let (started_at, ready_since, queued_since, session) = self.task.activity.poll_started(worker.id(), || self.task.poll_timestamps());
        self.update_poll_metrics(worker, started_at, ready_since);
        let mut poll = worker.task_poll_started_recorded(self.id(), started_at, queued_since, session);
        poll.task = session.map(|_| Arc::clone(&self.task));
        poll
    }

    // The assembly probe confirms these factored lifetime operations otherwise
    // become extra cross-crate calls in the existing disabled hot path.
    #[inline]
    fn update_poll_metrics(&self, worker: &WorkerHandle, started_at: EventTimestamp, ready_since: u64) {
        self.task.last_worker_id.store(worker.id().get(), Ordering::Release);
        let previous_poll_finished = self.task.last_poll_finished_at.swap(0, Ordering::AcqRel);
        if previous_poll_finished != 0 {
            let resume_nanos = duration_nanos(started_at, EventTimestamp::from_ticks(previous_poll_finished));
            self.task.resume_count.fetch_add(1, Ordering::Relaxed);
            self.task.resume_duration_nanos.fetch_add(resume_nanos, Ordering::Relaxed);
            self.task.max_resume_duration_nanos.fetch_max(resume_nanos, Ordering::Relaxed);
        }

        let ready_since = (ready_since != 0).then_some(EventTimestamp::from_ticks(ready_since));
        if let Some(ready_since) = ready_since {
            let ready_wait_nanos = duration_nanos(started_at, ready_since);
            self.task.ready_wait_count.fetch_add(1, Ordering::Relaxed);
            self.task.ready_wait_duration_nanos.fetch_add(ready_wait_nanos, Ordering::Relaxed);
            self.task
                .max_ready_wait_duration_nanos
                .fetch_max(ready_wait_nanos, Ordering::Relaxed);
        }
    }

    /// Finishes a poll and updates this task's lifetime counters.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the token prevents callers from finishing one poll twice"
    )]
    pub fn poll_finished(&self, worker: &WorkerHandle, poll: TaskPoll) {
        worker.task_poll_finished_with_control(&poll, Some(&self.task));
    }
}

impl TaskControl {
    #[inline]
    pub(crate) fn wake_timestamp(&self) -> EventTimestamp {
        let ready_since = EventTimestamp::now().ticks().max(1);
        let _already_ready = self
            .ready_since
            .compare_exchange(0, ready_since, Ordering::Release, Ordering::Relaxed);
        EventTimestamp::from_ticks(ready_since)
    }

    #[inline]
    pub(crate) fn poll_timestamps(&self) -> (EventTimestamp, u64) {
        let ready_since = self.ready_since.swap(0, Ordering::AcqRel);
        (EventTimestamp::now(), ready_since)
    }
}

/// Token pairing a task poll's start and finish events.
#[derive(Debug)]
#[must_use = "finish the poll with WorkerHandle::task_poll_finished"]
pub struct TaskPoll {
    pub(crate) task_id: TaskId,
    pub(crate) started_at: EventTimestamp,
    pub(crate) session: Option<RecordingSession>,
    pub(crate) task: Option<Arc<TaskControl>>,
}

impl TaskPoll {
    /// Start of this measured poll in the recorder clock.
    #[must_use]
    pub const fn started_at(&self) -> EventTimestamp {
        self.started_at
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeasurementSession {
    Recording(RecordingSession),
    Live(u64),
}

#[derive(Debug)]
struct Measurements {
    session: MeasurementSession,
    since: EventTimestamp,
    metrics: crate::snapshot::TaskMetrics,
}

/// Task instrumentation owned inline by the runtime's actual task.
///
/// Unlike [`TaskHandle`], this does not allocate a diagnostic control or maintain
/// a second task list. Poll clocks and measurement storage are used only during
/// runtime-event recording or a live subscription requesting poll timing.
#[derive(Debug)]
pub struct LiveTask {
    runtime: RuntimeHandle,
    id: TaskId,
    parent: Option<TaskId>,
    descriptor: TypeDescriptorId,
    size: u64,
    terminal: bool,
    measurements: Option<Box<Measurements>>,
}

impl LiveTask {
    /// Releases explicitly requested instrumentation without reporting a task outcome.
    ///
    /// Use this when recording stops while the actual task is still alive.
    /// Past observed counters are incomplete, not a claim that the task was canceled.
    pub fn stop_observing(mut self) {
        self.terminal = true;
    }

    pub(crate) fn new(runtime: RuntimeHandle, descriptor: TypeDescriptorId, parent: Option<TaskId>, size: usize) -> Self {
        let size = checked_size(size);
        let id = spawn_inline(&runtime, descriptor, parent, size);
        Self::from_registration(runtime, id, descriptor, parent, size)
    }

    fn from_registration(runtime: RuntimeHandle, id: TaskId, descriptor: TypeDescriptorId, parent: Option<TaskId>, size: u64) -> Self {
        Self {
            runtime,
            id,
            parent,
            descriptor,
            size,
            terminal: false,
            measurements: None,
        }
    }

    /// Returns the process-monotonic task identity.
    #[must_use]
    #[inline]
    pub const fn id(&self) -> TaskId {
        self.id
    }

    /// Returns the runtime owning this native task.
    #[must_use]
    #[inline]
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime.id()
    }

    /// Records initial enqueue/readiness only when event recording was requested.
    #[inline]
    pub fn enqueued(&self, worker: WorkerId) {
        record_enqueue(&self.runtime, self.id, worker);
    }

    /// Records materialization without allocating task metadata.
    #[inline]
    pub fn materialized(&self, worker: WorkerId) {
        self.runtime.task_materialized(self.id, worker);
    }

    fn session(&self, respect_suppression: bool) -> Option<MeasurementSession> {
        let generation = self.runtime.control.live_timing_generation.load(Ordering::Acquire);
        if generation != 0 {
            return Some(MeasurementSession::Live(generation));
        }
        let enabled = !respect_suppression || seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask);
        enabled
            .then(seismograph::recorder::active_recording_session)
            .flatten()
            .map(MeasurementSession::Recording)
    }

    /// Starts a poll, publishing its identity even if measurement collection is disabled.
    #[inline]
    pub fn poll_started(&mut self, worker: &WorkerHandle) -> LivePoll {
        let Some(session) = self.session(true) else {
            worker.task_entered(self.id);
            self.measurements = None;
            return LivePoll { poll: None, session: None };
        };
        let poll = worker.task_poll_started(self.id);
        if self
            .measurements
            .as_ref()
            .is_none_or(|measurements| measurements.session != session)
        {
            let _suppression = seismograph::recorder::SuppressionGuard::enter();
            self.measurements = Some(Box::new(Measurements {
                session,
                since: poll.started_at,
                metrics: crate::snapshot::TaskMetrics::default(),
            }));
        }
        LivePoll {
            poll: Some(poll),
            session: Some(session),
        }
    }

    /// Finishes a poll and updates only measurements from the same observation window.
    #[inline]
    pub fn poll_finished(&mut self, worker: &WorkerHandle, poll: LivePoll) {
        let Some(poll_token) = poll.poll else {
            worker.task_left();
            return;
        };
        let finished_at = EventTimestamp::now();
        if poll.session == self.session(true)
            && let Some(measurements) = &mut self.measurements
        {
            let duration = duration_nanos(finished_at, poll_token.started_at);
            measurements.metrics.poll_count = measurements.metrics.poll_count.saturating_add(1);
            measurements.metrics.poll_duration_nanos = measurements.metrics.poll_duration_nanos.saturating_add(duration);
            measurements.metrics.max_poll_duration_nanos = measurements.metrics.max_poll_duration_nanos.max(duration);
        }
        worker.task_poll_finished_at(&poll_token, None, finished_at);
    }

    /// Builds an owned description when the runtime inspects its native task queues.
    ///
    /// Queue state does not imply a known Ready age. Live producers do not collect
    /// creation times, per-wake timestamps, or resume metrics. Requested spawn
    /// events retain their own timestamps independently of this live description.
    #[must_use]
    pub fn snapshot(&self, state: crate::snapshot::TaskActivityState, worker: Option<WorkerId>) -> crate::snapshot::Task {
        let measurements = self
            .measurements
            .as_ref()
            .filter(|measurements| Some(measurements.session) == self.session(false));
        crate::snapshot::Task {
            id: self.id,
            parent: self.parent,
            type_descriptor: self.descriptor,
            future_size_bytes: Some(self.size),
            spawned_at: EventTimestamp::from_ticks(0),
            last_worker_id: worker,
            metrics: measurements.map_or_else(crate::snapshot::TaskMetrics::default, |measurements| measurements.metrics),
            metrics_scope: measurements.map_or(crate::snapshot::TaskMetricsScope::Uncollected, |measurements| {
                crate::snapshot::TaskMetricsScope::PollsSince(measurements.since)
            }),
            activity: Some(crate::snapshot::TaskActivity {
                observed_at: EventTimestamp::now(),
                state,
                ready_since: None,
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: None,
            }),
            spawn_backtrace: Vec::new(),
        }
    }

    /// Reports successful completion at most once.
    #[inline]
    pub fn completed(&mut self, worker: Option<WorkerId>) {
        self.finish(worker, TaskOutcome::Completed);
    }

    /// Reports cancellation at most once.
    #[inline]
    pub fn canceled(&mut self, worker: Option<WorkerId>) {
        self.finish(worker, TaskOutcome::Canceled);
    }

    /// Reports a panic at most once.
    #[inline]
    pub fn panicked(&mut self, worker: Option<WorkerId>) {
        self.finish(worker, TaskOutcome::Panicked);
    }

    fn finish(&mut self, worker: Option<WorkerId>, outcome: TaskOutcome) {
        if std::mem::replace(&mut self.terminal, true) {
            return;
        }
        finish_inline(&self.runtime, self.id, worker, outcome);
    }
}

fn finish_inline(runtime: &RuntimeHandle, id: TaskId, worker: Option<WorkerId>, outcome: TaskOutcome) {
    let (kind, counter, backtrace) = match outcome {
        TaskOutcome::Completed => (
            EventKind::TaskCompleted,
            &runtime.control.counters.completed_tasks,
            BacktraceCapture::Never,
        ),
        TaskOutcome::Canceled => (
            EventKind::TaskCanceled,
            &runtime.control.counters.canceled_tasks,
            BacktraceCapture::Never,
        ),
        TaskOutcome::Panicked => (
            EventKind::TaskPanicked,
            &runtime.control.counters.panicked_tasks,
            runtime.control.lifecycle_backtraces,
        ),
    };
    crate::decrement_saturating(&runtime.control.counters.live_tasks);
    counter.fetch_add(1, Ordering::Relaxed);
    crate::record_now(&runtime.control, worker, kind, id.get(), 0, 0, 0, backtrace);
}

impl Drop for LiveTask {
    fn drop(&mut self) {
        self.canceled(None);
    }
}

/// Token pairing an optionally measured native task poll.
#[derive(Debug)]
#[must_use = "finish the native task poll with LiveTask::poll_finished"]
pub struct LivePoll {
    poll: Option<TaskPoll>,
    session: Option<MeasurementSession>,
}

/// Compact instrumentation owned by a submitted, not-yet-materialized factory.
///
/// Dropping this token reports cancellation. Materialization transfers ownership
/// into the actual task, without allocating or registering a diagnostic control.
#[derive(Debug)]
pub struct PendingTask {
    worker: Option<WorkerHandle>,
    id: Option<TaskId>,
    parent: Option<TaskId>,
    descriptor: TypeDescriptorId,
    size: u64,
}

impl PendingTask {
    /// Releases a submitted task's instrumentation without inventing cancellation.
    pub fn stop_observing(mut self) {
        self.id = None;
    }

    pub(crate) fn new(worker: WorkerHandle, descriptor: TypeDescriptorId, parent: Option<TaskId>, size: usize) -> Self {
        let size = checked_size(size);
        let id = spawn_inline(worker.runtime(), descriptor, parent, size);
        Self {
            worker: Some(worker),
            id: Some(id),
            parent,
            descriptor,
            size,
        }
    }

    /// Records submission/readiness without constructing poll measurement storage.
    ///
    /// # Panics
    ///
    /// Panics only if an internal token-ownership invariant is violated.
    #[inline]
    pub fn enqueued(&self) {
        let worker = self
            .worker
            .as_ref()
            .expect("pending tokens retain their worker until materialization");
        record_enqueue(
            worker.runtime(),
            self.id.expect("pending tokens retain their task identity"),
            worker.id(),
        );
    }

    /// Transfers this registration to the actual task and its worker.
    ///
    /// # Panics
    ///
    /// Panics only if an internal token-ownership invariant is violated.
    #[must_use]
    #[inline]
    pub fn materialized(mut self) -> (LiveTask, WorkerHandle) {
        let worker = self.worker.take().expect("pending tokens materialize exactly once");
        let id = self.id.take().expect("pending tokens transfer their identity exactly once");
        let task = LiveTask::from_registration(worker.runtime().clone(), id, self.descriptor, self.parent, self.size);
        task.materialized(worker.id());
        (task, worker)
    }

    /// Reports a factory panic before materialization.
    pub fn panicked(mut self) {
        self.finish(TaskOutcome::Panicked);
    }

    fn finish(&mut self, outcome: TaskOutcome) {
        if let Some(id) = self.id.take() {
            let worker = self.worker.as_ref().expect("unmaterialized tokens retain their worker");
            finish_inline(worker.runtime(), id, None, outcome);
        }
    }
}

impl Drop for PendingTask {
    fn drop(&mut self) {
        self.finish(TaskOutcome::Canceled);
    }
}

fn checked_size(size: usize) -> u64 {
    let size = u64::try_from(size).expect("supported targets have pointer widths no larger than 64 bits");
    assert!(size != u64::MAX, "a task future's size must be smaller than u64::MAX");
    size
}

fn spawn_inline(runtime: &RuntimeHandle, descriptor: TypeDescriptorId, parent: Option<TaskId>, size: u64) -> TaskId {
    let id = crate::next_task_id();
    runtime.control.counters.spawned_tasks.fetch_add(1, Ordering::Relaxed);
    runtime.control.counters.live_tasks.fetch_add(1, Ordering::Relaxed);
    crate::record_now(
        &runtime.control,
        None,
        EventKind::TaskSpawned,
        id.get(),
        parent.map_or(0, TaskId::get),
        descriptor.get(),
        size.checked_add(1).expect("future sizes were validated before registration"),
        runtime.control.lifecycle_backtraces,
    );
    id
}

#[inline]
fn record_enqueue(runtime: &RuntimeHandle, id: TaskId, worker: WorkerId) {
    runtime.task_enqueued(id, Some(worker));
    crate::record_now(
        &runtime.control,
        None,
        EventKind::TaskReady,
        id.get(),
        0,
        0,
        0,
        BacktraceCapture::Never,
    );
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
    use seismograph::snapshot::SnapshotOptions;

    use super::*;
    use crate::live::{Measurements as RequestedMeasurements, SubscriptionOptions, subscribe};
    use crate::snapshot::{TaskActivityState, TaskMetricsScope};
    use crate::worker::{WorkerMetadata, WorkerRole};
    use crate::{RuntimeMetadata, allocate_task_id, allocate_type_descriptor_id, register_runtime};

    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            seismograph::recorder(Configuration::default());
        }
    }

    fn record_tasks() -> Reset {
        seismograph::recorder(Configuration {
            runtime_tasks: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(256).unwrap(),
            ..Configuration::default()
        });
        Reset
    }

    fn events_for(runtime: RuntimeId) -> Vec<seismograph::recorder::event::Event> {
        let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        seismograph::snapshot::decode(bytes.as_bytes())
            .unwrap()
            .events
            .events
            .into_iter()
            .filter(|event| event.runtime().is_some_and(|event| event.runtime_id == runtime))
            .collect()
    }

    #[test]
    fn inline_lifecycle_preserves_metadata_events_and_exactly_one_terminal_outcome() {
        let _test = crate::tests::test_lock();
        for outcome in [TaskOutcome::Completed, TaskOutcome::Canceled, TaskOutcome::Panicked] {
            let _reset = record_tasks();
            let runtime = register_runtime(RuntimeMetadata::new("inline-outcome", 1));
            let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
            let descriptor = allocate_type_descriptor_id();
            let parent = allocate_task_id();
            let mut task = runtime.handle().register_live_task(descriptor, Some(parent), 17);
            let id = task.id();
            task.enqueued(worker.id());
            task.materialized(worker.id());
            let description = task.snapshot(TaskActivityState::Waiting, Some(worker.id()));
            assert_eq!(
                (
                    task.runtime_id(),
                    description.id,
                    description.parent,
                    description.type_descriptor,
                    description.future_size_bytes
                ),
                (runtime.id(), id, Some(parent), descriptor, Some(17))
            );
            let (kind, completed, canceled, panicked) = match outcome {
                TaskOutcome::Completed => {
                    task.completed(Some(worker.id()));
                    (EventKind::TaskCompleted, 1, 0, 0)
                }
                TaskOutcome::Canceled => {
                    task.canceled(Some(worker.id()));
                    (EventKind::TaskCanceled, 0, 1, 0)
                }
                TaskOutcome::Panicked => {
                    task.panicked(Some(worker.id()));
                    (EventKind::TaskPanicked, 0, 0, 1)
                }
            };
            task.completed(Some(worker.id()));
            drop(task);
            let counters = runtime.counters().snapshot();
            assert_eq!(
                (
                    counters.spawned_tasks,
                    counters.live_tasks,
                    counters.completed_tasks,
                    counters.canceled_tasks,
                    counters.panicked_tasks
                ),
                (1, 0, completed, canceled, panicked)
            );
            let events = events_for(runtime.id());
            for expected in [
                EventKind::TaskSpawned,
                EventKind::TaskEnqueued,
                EventKind::TaskReady,
                EventKind::TaskMaterialized,
                kind,
            ] {
                assert_eq!(events.iter().filter(|event| event.kind == expected).count(), 1);
            }
        }
    }

    #[test]
    fn dropping_or_stopping_inline_observation_does_not_confuse_cancellation_with_disconnection() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("inline-drop", 0));
        drop(runtime.handle().register_live_task(allocate_type_descriptor_id(), None, 0));
        assert_eq!(runtime.counters().snapshot().canceled_tasks, 1);
        let task = runtime.handle().register_live_task(allocate_type_descriptor_id(), None, 1);
        let before = runtime.counters().snapshot();
        task.stop_observing();
        assert_eq!(runtime.counters().snapshot(), before);
    }

    #[test]
    fn pending_tokens_transfer_identity_and_report_cancel_or_panic_only_once() {
        let _test = crate::tests::test_lock();
        let _reset = record_tasks();
        let runtime = register_runtime(RuntimeMetadata::new("pending-transitions", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let descriptor = allocate_type_descriptor_id();
        let parent = allocate_task_id();
        let pending = worker.handle().prepare_live_task(descriptor, Some(parent), 19);
        let id = pending.id.unwrap();
        pending.enqueued();
        let (mut task, owner) = pending.materialized();
        assert_eq!((task.id(), owner.id()), (id, worker.id()));
        assert_eq!(
            task.snapshot(TaskActivityState::Ready, Some(owner.id())).future_size_bytes,
            Some(19)
        );
        task.completed(Some(owner.id()));
        drop(task);
        assert_eq!(runtime.counters().snapshot().canceled_tasks, 0);
        drop(worker.handle().prepare_live_task(descriptor, None, 0));
        worker.handle().prepare_live_task(descriptor, None, 1).panicked();
        let pending = worker.handle().prepare_live_task(descriptor, None, 2);
        let before = runtime.counters().snapshot();
        pending.stop_observing();
        assert_eq!(runtime.counters().snapshot(), before);
        let counters = runtime.counters().snapshot();
        assert_eq!(
            (
                counters.completed_tasks,
                counters.canceled_tasks,
                counters.panicked_tasks,
                counters.live_tasks
            ),
            (1, 1, 1, 1)
        );
        let events = events_for(runtime.id());
        for kind in [EventKind::TaskEnqueued, EventKind::TaskReady, EventKind::TaskMaterialized] {
            assert_eq!(events.iter().filter(|event| event.kind == kind).count(), 1);
        }
    }

    #[test]
    fn inline_recording_measurements_accumulate_and_restart_in_a_new_recording_session() {
        let _test = crate::tests::test_lock();
        let _reset = record_tasks();
        let runtime = register_runtime(RuntimeMetadata::new("recording-windows", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let owner = worker.handle();
        let mut task = runtime.handle().register_live_task(allocate_type_descriptor_id(), None, 7);
        for expected in 1..=2 {
            let poll = task.poll_started(&owner);
            std::thread::sleep(Duration::from_millis(1));
            task.poll_finished(&owner, poll);
            let snapshot = task.snapshot(TaskActivityState::Waiting, Some(owner.id()));
            assert_eq!(snapshot.metrics.poll_count, expected);
            assert!(snapshot.metrics.poll_duration_nanos > 1);
            assert!(snapshot.metrics.max_poll_duration_nanos > 1);
            assert!(matches!(snapshot.metrics_scope, TaskMetricsScope::PollsSince(_)));
            let _suppression = seismograph::recorder::SuppressionGuard::enter();
            assert_eq!(
                task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics,
                snapshot.metrics
            );
        }
        seismograph::recorder(Configuration::default());
        assert_eq!(
            task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics_scope,
            TaskMetricsScope::Uncollected
        );
        let poll = task.poll_started(&owner);
        task.poll_finished(&owner, poll);
        let _new_session = record_tasks();
        let poll = task.poll_started(&owner);
        task.poll_finished(&owner, poll);
        assert_eq!(task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics.poll_count, 1);
    }

    #[test]
    fn requested_live_timing_resets_windows_and_does_not_accept_a_poll_from_an_old_generation() {
        let _test = crate::tests::test_lock();
        seismograph::recorder(Configuration::default());
        let runtime = register_runtime(RuntimeMetadata::new("live-windows", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let owner = worker.handle();
        let mut task = runtime.handle().register_live_task(allocate_type_descriptor_id(), None, 3);
        let options = SubscriptionOptions {
            measurements: RequestedMeasurements::PollTiming,
            ..SubscriptionOptions::default()
        };
        let first = subscribe(runtime.id(), options, |_| {}).unwrap();
        let first_generation = owner.live_timing_generation();
        assert!(first_generation > 0);
        let poll = task.poll_started(&owner);
        std::thread::sleep(Duration::from_millis(1));
        task.poll_finished(&owner, poll);
        assert_eq!(task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics.poll_count, 1);
        let old_poll = task.poll_started(&owner);
        drop(first);
        let second = subscribe(runtime.id(), options, |_| {}).unwrap();
        assert!(owner.live_timing_generation() > first_generation);
        task.poll_finished(&owner, old_poll);
        assert_eq!(
            task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics_scope,
            TaskMetricsScope::Uncollected
        );
        let poll = task.poll_started(&owner);
        task.poll_finished(&owner, poll);
        assert_eq!(task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics.poll_count, 1);
        drop(second);
        let poll = task.poll_started(&owner);
        task.poll_finished(&owner, poll);
        assert_eq!(
            task.snapshot(TaskActivityState::Waiting, Some(owner.id())).metrics_scope,
            TaskMetricsScope::Uncollected
        );
    }

    #[test]
    fn finishing_a_requested_worker_poll_returns_the_actual_recorded_duration() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("measured-result", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let poll = worker.handle().task_poll_started(allocate_task_id());
        std::thread::sleep(Duration::from_millis(1));
        let elapsed = worker.handle().task_poll_finished_measured(poll);
        assert!(elapsed > 1);
        assert_eq!(elapsed, runtime.counters().snapshot().poll_duration_nanos);
    }
}
