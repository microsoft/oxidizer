// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker registration and worker-local runtime instrumentation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp};
use seismograph::recorder::runtime::{TaskId, TransferId, WorkerId};
use seismograph::recorder::{is_enabled, without_recording};

use crate::snapshot::WorkerState;
use crate::task::TaskPoll;
use crate::{RuntimeHandle, WorkerControl, duration_nanos, lock, next_transfer_id, record_at, record_now};

/// Functional role assigned to a runtime worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkerRole {
    /// Executes general runtime tasks.
    Core,
    /// Executes blocking work.
    Blocking,
    /// Drives runtime I/O.
    Io,
}

impl WorkerRole {
    pub(crate) const fn wire_value(self) -> u8 {
        match self {
            Self::Core => 1,
            Self::Blocking => 2,
            Self::Io => 3,
        }
    }

    pub(crate) const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Core),
            2 => Some(Self::Blocking),
            3 => Some(Self::Io),
            _ => None,
        }
    }
}

/// Metadata retained for one runtime worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerMetadata {
    /// Functional role assigned to the worker.
    pub(crate) role: WorkerRole,
    /// Logical processor selected by affinity configuration, when known.
    pub(crate) processor_index: Option<u32>,
}

impl WorkerMetadata {
    /// Creates worker metadata without an affinity processor.
    #[must_use]
    pub const fn new(role: WorkerRole) -> Self {
        Self {
            role,
            processor_index: None,
        }
    }

    /// Associates the worker with a logical processor.
    #[must_use]
    pub const fn processor_index(mut self, processor_index: u32) -> Self {
        self.processor_index = Some(processor_index);
        self
    }
}

/// RAII registration for one runtime worker.
///
/// Dropping the registration marks the worker stopped while retaining its
/// metadata in the owning runtime record.
#[derive(Debug)]
pub struct WorkerRegistration {
    handle: WorkerHandle,
}

impl WorkerRegistration {
    pub(crate) fn new(runtime: RuntimeHandle, worker: Arc<WorkerControl>) -> Self {
        Self {
            handle: WorkerHandle { runtime, worker },
        }
    }

    /// Returns the process-monotonic worker identity.
    #[must_use]
    pub fn id(&self) -> WorkerId {
        self.handle.id()
    }

    /// Creates a cheap worker handle for hot-path instrumentation.
    #[must_use]
    pub fn handle(&self) -> WorkerHandle {
        self.handle.clone()
    }

    /// Associates the current Seismograph recorder thread with this worker.
    pub fn attach_current_thread(&self) {
        self.handle.attach_current_thread();
    }
}

impl Drop for WorkerRegistration {
    fn drop(&mut self) {
        self.handle.stop();
    }
}

/// Cheap shared handle for worker-local task and transfer events.
#[derive(Clone, Debug)]
pub struct WorkerHandle {
    runtime: RuntimeHandle,
    worker: Arc<WorkerControl>,
}

impl WorkerHandle {
    pub(crate) const fn new(runtime: RuntimeHandle, worker: Arc<WorkerControl>) -> Self {
        Self { runtime, worker }
    }

    /// Copies task-event identities and policy without cloning shared handles.
    #[must_use]
    pub fn task_events(&self) -> crate::task::EventContext {
        crate::task::EventContext::new(self.runtime.id(), Some(self.worker.id), self.runtime.control.lifecycle_backtraces)
    }

    /// Returns whether the next execution cycle needs diagnostic poll hooks.
    ///
    /// Check once outside the normal poll loop. When false, do not call the
    /// per-poll instrumentation methods. This query does not read a clock or
    /// mutate diagnostic state.
    #[must_use]
    #[inline]
    pub fn is_observed(&self) -> bool {
        self.runtime.control.requests.load(Ordering::Relaxed) != 0 || is_enabled(EventClass::RuntimeTask)
    }

    /// Routes bounded task requests to this worker's native storage.
    ///
    /// The callback runs on the diagnostic caller and must return promptly.
    /// Queue the request for the worker and wake it. The worker responds between
    /// execution cycles, after copying descriptions out of its native storage.
    ///
    /// # Errors
    ///
    /// Returns an error when the worker already stopped.
    pub fn register_task_provider(
        &self,
        provider: impl Fn(crate::task::TaskRequest) + Send + Sync + 'static,
    ) -> Result<(), seismograph::Error> {
        without_recording(|| {
            let mut slot = lock(&self.worker.provider);
            if self.worker.state.load(Ordering::Acquire) == WorkerState::Stopped.wire_value() {
                return Err(seismograph::Error::new("cannot register a task provider on a stopped worker"));
            }
            let previous = slot.replace(Arc::new(provider));
            drop(slot);
            drop(previous);
            Ok(())
        })
    }

    /// Returns the process-monotonic worker identity.
    #[must_use]
    pub fn id(&self) -> WorkerId {
        self.worker.id
    }

    /// Associates the current Seismograph recorder thread with this worker.
    pub fn attach_current_thread(&self) {
        let thread_id = seismograph::recorder::thread_id();
        self.worker.thread_id.store(thread_id.get(), Ordering::Release);
    }

    /// Marks this worker parked and emits a high-frequency event without a backtrace.
    #[inline]
    pub fn parked(&self) {
        self.worker.state.store(WorkerState::Parked.wire_value(), Ordering::Release);
        self.record(EventKind::WorkerParked, self.id().get(), 0, 0, 0, BacktraceCapture::Never);
    }

    /// Marks this worker running and emits a high-frequency event without a backtrace.
    #[inline]
    pub fn unparked(&self) {
        self.worker.state.store(WorkerState::Running.wire_value(), Ordering::Release);
        self.record(EventKind::WorkerUnparked, self.id().get(), 0, 0, 0, BacktraceCapture::Never);
    }

    /// Starts a task poll and associates the task with this worker.
    #[inline]
    pub fn task_poll_started(&self, task_id: TaskId) -> TaskPoll {
        self.poll_started_with(task_id, EventTimestamp::now)
    }

    pub(crate) fn poll_started_with(&self, task_id: TaskId, now: impl FnOnce() -> EventTimestamp) -> TaskPoll {
        // Acquire pairs with the observer's Release publication of the active window.
        let window = self.runtime.control.timing_window.load(Ordering::Acquire);
        let started_at = (window != 0 || is_enabled(EventClass::RuntimeTask)).then(now);
        self.worker.current_task.store(task_id.get(), Ordering::Release);
        if let Some(at) = started_at {
            self.record_at(at, EventKind::TaskPollStarted, task_id.get(), 0, 0, 0, BacktraceCapture::Never);
        }
        TaskPoll {
            task: task_id,
            worker: Arc::clone(&self.worker),
            started_at,
            window,
            active: true,
        }
    }

    /// Finishes an observed poll without retaining task state.
    ///
    /// Only a still-active measurement window receives poll aggregates.
    ///
    /// # Panics
    ///
    /// Panics if the token belongs to another worker.
    #[inline]
    pub fn task_poll_finished(&self, poll: TaskPoll) {
        self.poll_finished_with(poll, EventTimestamp::now);
    }

    pub(crate) fn poll_finished_with(&self, mut poll: TaskPoll, now: impl FnOnce() -> EventTimestamp) {
        assert_eq!(poll.worker.id, self.id(), "finish a poll on the worker that started it");
        poll.active = false;
        self.worker.current_task.store(0, Ordering::Release);
        let Some(started_at) = poll.started_at else { return };
        let finished_at = now();
        let elapsed = duration_nanos(finished_at, started_at);
        // #[gamma::skip(logical.and_remove_right, reason = "collect_poll_measurement rechecks the same window under the measurements lock before adding data; window identities are never reused, so removing this outer fast-path comparison only takes a redundant lock for stale polls")]
        if poll.window != 0 && self.runtime.control.timing_window.load(Ordering::Acquire) == poll.window {
            without_recording(|| {
                self.collect_poll_measurement(&poll, started_at, elapsed);
            });
        }
        self.record_at(
            finished_at,
            EventKind::TaskPollFinished,
            poll.task.get(),
            0,
            elapsed,
            0,
            BacktraceCapture::Never,
        );
    }

    fn collect_poll_measurement(&self, poll: &TaskPoll, started_at: EventTimestamp, elapsed: u64) {
        let mut measurements = lock(&self.worker.measurements);
        // The last observer publishes zero before clearing under this lock.
        // Recheck here so a concurrent finish cannot refill a retired window.
        // #[gamma::skip(cond.always_true, tag = "equivalent", reason = "Measurements::add rechecks this same window before changing any state; retired window identities are never reused")]
        if self.runtime.control.timing_window.load(Ordering::Acquire) == poll.window {
            measurements.add(poll.window, &self.runtime.control.timing_window, poll.task, started_at, elapsed);
        }
    }

    /// Starts a task instance transfer from this worker.
    #[inline]
    pub fn transfer_started(&self, task_id: TaskId, destination: WorkerId) -> Transfer {
        let transfer_id = next_transfer_id();
        let started_at = EventTimestamp::now();
        self.record_at(
            started_at,
            EventKind::TransferStarted,
            transfer_id.get(),
            task_id.get(),
            destination.get(),
            self.id().get(),
            BacktraceCapture::Never,
        );
        Transfer {
            id: transfer_id,
            task_id,
            source: self.id(),
            destination,
            started_at,
        }
    }

    /// Emits an instance relocation associated with `transfer`.
    #[inline]
    pub fn instance_relocated(&self, transfer: &Transfer) {
        self.record(
            EventKind::InstanceRelocated,
            transfer.id.get(),
            transfer.task_id.get(),
            transfer.source.get(),
            transfer.destination.get(),
            BacktraceCapture::Never,
        );
    }

    /// Finishes an instance transfer and records its duration.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the token prevents callers from finishing one transfer twice"
    )]
    pub fn transfer_finished(&self, transfer: Transfer) {
        let finished_at = EventTimestamp::now();
        self.record_at(
            finished_at,
            EventKind::TransferFinished,
            transfer.id.get(),
            transfer.task_id.get(),
            duration_nanos(finished_at, transfer.started_at),
            transfer.destination.get(),
            BacktraceCapture::Never,
        );
    }

    pub(crate) fn stop(&self) {
        drop(self.detach_provider());
    }

    pub(crate) fn detach_provider(&self) -> Option<crate::TaskProvider> {
        let mut slot = lock(&self.worker.provider);
        self.worker.current_task.store(0, Ordering::Relaxed);
        let previous = self.worker.state.swap(WorkerState::Stopped.wire_value(), Ordering::AcqRel);
        let provider = slot.take();
        drop(slot);
        if previous != WorkerState::Stopped.wire_value() {
            self.record(
                EventKind::WorkerStopped,
                self.id().get(),
                0,
                0,
                0,
                self.runtime.control.lifecycle_backtraces,
            );
        }
        provider
    }

    #[inline]
    fn record(&self, kind: EventKind, subject_id: u64, related_id: u64, value_0: u64, value_1: u64, backtrace: BacktraceCapture) {
        record_now(
            &self.runtime.control,
            Some(self.worker.id),
            kind,
            subject_id,
            related_id,
            value_0,
            value_1,
            backtrace,
        );
    }

    #[inline]
    #[expect(
        clippy::too_many_arguments,
        reason = "The fixed runtime event payload has two identities and two numeric values"
    )]
    fn record_at(
        &self,
        timestamp: EventTimestamp,
        kind: EventKind,
        subject_id: u64,
        related_id: u64,
        value_0: u64,
        value_1: u64,
        backtrace: BacktraceCapture,
    ) {
        record_at(
            &self.runtime.control,
            timestamp,
            Some(self.worker.id),
            kind,
            subject_id,
            related_id,
            value_0,
            value_1,
            backtrace,
        );
    }
}

/// Token pairing an instance transfer's lifecycle events.
#[derive(Debug)]
#[must_use = "finish the transfer with WorkerHandle::transfer_finished"]
pub struct Transfer {
    id: TransferId,
    task_id: TaskId,
    source: WorkerId,
    destination: WorkerId,
    started_at: EventTimestamp,
}

#[cfg(test)]
pub(crate) mod gamma_round2_tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU8, AtomicU64};

    use super::*;

    #[test]
    fn a_window_retired_after_the_fast_path_check_cannot_be_refilled_under_the_measurement_lock() {
        without_recording(|| {
            let handle = worker();
            handle.runtime.control.timing_window.store(1, Ordering::Release);
            let task = TaskId::from_raw(23).unwrap();
            let started_at = EventTimestamp::from_ticks(10);
            let poll = handle.poll_started_with(task, || started_at);
            handle.collect_poll_measurement(&poll, started_at, 37);
            assert_eq!(lock(&handle.worker.measurements).tasks[&task].metrics.poll_duration_nanos, 37);
            lock(&handle.worker.measurements).tasks.clear();

            for retired_window in [0, 2] {
                handle.runtime.control.timing_window.store(retired_window, Ordering::Release);
                handle.collect_poll_measurement(&poll, started_at, 37);
                assert!(lock(&handle.worker.measurements).tasks.is_empty());
            }
        });
    }

    #[test]
    fn finishing_a_poll_does_not_clear_a_new_poll_started_during_the_finish_clock_read() {
        without_recording(|| {
            let handle = worker();
            handle.runtime.control.timing_window.store(1, Ordering::Release);
            let task = TaskId::from_raw(23).unwrap();
            let poll = handle.poll_started_with(task, || EventTimestamp::from_ticks(10));
            let mut next = None;
            handle.poll_finished_with(poll, || {
                next = Some(handle.poll_started_with(task, || EventTimestamp::from_ticks(30)));
                EventTimestamp::from_ticks(20)
            });
            assert_eq!(handle.worker.current_task.load(Ordering::Acquire), task.get());
            drop(next);
            assert_eq!(handle.worker.current_task.load(Ordering::Acquire), 0);
        });
    }

    pub(crate) fn runtime() -> RuntimeHandle {
        worker().runtime
    }

    #[test]
    fn unrecorded_poll_start_still_tracks_current_task() {
        without_recording(|| {
            let worker = worker();
            let task = TaskId::from_raw(7).unwrap();
            let poll = worker.poll_started_with(task, || panic!("unobserved polls need no clock"));
            assert_eq!(poll.task, task);
            assert_eq!(poll.started_at, None);
            assert_eq!(worker.worker.current_task.load(Ordering::Acquire), task.get());
            drop(poll);
            assert_eq!(worker.worker.current_task.load(Ordering::Acquire), 0);
        });
    }

    #[test]
    fn disabled_hooks_leave_recording_state_and_event_buffers_untouched() {
        let _test = crate::tests::test_lock();
        seismograph::recorder(seismograph::recorder::Configuration::default());
        seismograph::recorder::clear_buffers().unwrap();
        let worker = worker();
        let task = TaskId::from_raw(7).unwrap();
        for _ in 0..10 {
            let poll = worker.poll_started_with(task, || panic!("disabled poll start must not sample time"));
            worker.poll_finished_with(poll, || panic!("disabled poll finish must not sample time"));
        }
        assert!(lock(&worker.worker.measurements).tasks.is_empty());
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let captured = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        assert!(captured.events.events.is_empty());
    }

    #[test]
    fn gamma_round2_frozen_poll_finishes_accumulate_exact_durations_and_stop_once() {
        let _test = crate::tests::test_lock();
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::Policy::all(false),
            ..seismograph::recorder::Configuration::default()
        });
        seismograph::recorder::clear_buffers().unwrap();
        let worker = worker();
        let task = TaskId::from_raw(23).unwrap();
        worker.runtime.control.timing_window.store(1, Ordering::Release);
        for (start, finish) in [(30, 100), (0, 0), (1, 1)] {
            let poll = worker.poll_started_with(task, || EventTimestamp::from_ticks(start));
            worker.poll_finished_with(poll, || EventTimestamp::from_ticks(finish));
            assert_eq!(worker.worker.current_task.load(Ordering::Acquire), 0);
        }
        {
            let measurements = lock(&worker.worker.measurements);
            let measured = &measurements.tasks[&task];
            assert_eq!(measured.since, EventTimestamp::from_ticks(30));
            assert_eq!(measured.metrics.poll_count, 3);
            assert_eq!(measured.metrics.poll_duration_nanos, 70);
            assert_eq!(measured.metrics.max_poll_duration_nanos, 70);
        }
        worker.stop();
        worker.stop();
        let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        let starts = decoded
            .events
            .events
            .iter()
            .filter(|event| event.kind == EventKind::TaskPollStarted)
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 3);
        for (event, at) in starts.into_iter().zip([30, 0, 1]) {
            assert_eq!(event.timestamp.ticks(), at);
            assert_eq!(
                event.runtime().unwrap(),
                seismograph::recorder::runtime::RuntimeEvent {
                    runtime_id: worker.runtime.id(),
                    worker_id: Some(worker.id()),
                    subject_id: task.get(),
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                }
            );
        }
        let finishes = decoded
            .events
            .events
            .iter()
            .filter(|event| event.kind == EventKind::TaskPollFinished)
            .collect::<Vec<_>>();
        assert_eq!(finishes.len(), 3);
        for (event, duration) in finishes.into_iter().zip([70, 0, 0]) {
            assert_eq!(
                event.runtime().unwrap(),
                seismograph::recorder::runtime::RuntimeEvent {
                    runtime_id: worker.runtime.id(),
                    worker_id: Some(worker.id()),
                    subject_id: task.get(),
                    related_id: 0,
                    value_0: duration,
                    value_1: 0,
                }
            );
        }
        assert_eq!(
            decoded
                .events
                .events
                .iter()
                .filter(|event| event.kind == EventKind::WorkerStopped)
                .count(),
            1
        );
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }
    use crate::RuntimeControl;

    pub(crate) fn worker() -> WorkerHandle {
        WorkerHandle {
            runtime: RuntimeHandle {
                control: Arc::new(RuntimeControl {
                    id: seismograph::recorder::runtime::RuntimeId::from_raw(13).unwrap(),
                    name: "round2".into(),
                    configured_workers: 1,
                    lifecycle_backtraces: BacktraceCapture::Never,
                    created_at: EventTimestamp::from_ticks(17),
                    retired_at: AtomicU64::new(0),
                    state: AtomicU8::new(1),
                    workers: Mutex::new(Vec::new()),
                    requests: std::sync::atomic::AtomicUsize::new(0),
                    timing_window: AtomicU64::new(0),
                    timing: Mutex::new(crate::collection::Timing::default()),
                }),
            },
            worker: Arc::new(WorkerControl {
                id: WorkerId::from_raw(19).unwrap(),
                role: WorkerRole::Core,
                processor_index: None,
                state: AtomicU8::new(1),
                thread_id: AtomicU64::new(0),
                current_task: AtomicU64::new(0),
                provider: Mutex::new(None),
                request_pending: std::sync::atomic::AtomicBool::new(false),
                measurements: Mutex::new(crate::collection::Measurements::default()),
            }),
        }
    }

    #[test]
    fn gamma_round2_worker_snapshot_zero_sentinels_do_not_hide_present_identities() {
        let worker = worker();
        let empty = worker.worker.snapshot(crate::snapshot::TaskCollection::Unavailable);
        assert_eq!((empty.current_task, empty.thread_id), (None, None));
        worker.worker.current_task.store(23, Ordering::Relaxed);
        worker.worker.thread_id.store(29, Ordering::Relaxed);
        let full = worker.worker.snapshot(crate::snapshot::TaskCollection::Complete);
        assert_eq!(full.current_task.unwrap().get(), 23);
        assert_eq!(full.thread_id.unwrap().get(), 29);
        worker.worker.current_task.store(0, Ordering::Relaxed);
        assert_eq!(worker.worker.snapshot(crate::snapshot::TaskCollection::Complete).current_task, None);
    }

    #[test]
    fn gamma_round2_worker_registration_and_stop_clear_optional_execution_state() {
        without_recording(|| {
            let fixture = worker();
            let registration = fixture.runtime.register_worker(WorkerMetadata::new(WorkerRole::Blocking));
            let handle = registration.handle();
            let initial = handle.worker.snapshot(crate::snapshot::TaskCollection::Unavailable);
            assert_eq!((initial.thread_id, initial.current_task), (None, None));
            assert_eq!(
                (initial.role, initial.state, initial.processor_index),
                (WorkerRole::Blocking, WorkerState::Running, None)
            );
            let task = TaskId::from_raw(23).unwrap();
            handle.runtime.control.timing_window.store(1, Ordering::Release);
            let poll = handle.poll_started_with(task, || EventTimestamp::from_ticks(100));
            assert_eq!((poll.task, poll.started_at), (task, Some(EventTimestamp::from_ticks(100))));
            assert_eq!(
                handle.worker.snapshot(crate::snapshot::TaskCollection::Complete).current_task,
                Some(task)
            );
            drop(registration);
            let stopped = handle.worker.snapshot(crate::snapshot::TaskCollection::Stopped);
            assert_eq!(stopped.current_task, None);
            assert_eq!(stopped.state, WorkerState::Stopped);
            handle.stop();
            assert_eq!(handle.worker.snapshot(crate::snapshot::TaskCollection::Stopped).current_task, None);
        });
    }
}
