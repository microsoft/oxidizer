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
        let window = self.runtime.control.timing_window.load(Ordering::Acquire);
        let started_at = (window != 0 || is_enabled(EventClass::RuntimeTask)).then(EventTimestamp::now);
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
    pub fn task_poll_finished(&self, mut poll: TaskPoll) {
        assert_eq!(poll.worker.id, self.id(), "finish a poll on the worker that started it");
        poll.active = false;
        self.worker.current_task.store(0, Ordering::Release);
        let Some(started_at) = poll.started_at else { return };
        let finished_at = EventTimestamp::now();
        let elapsed = duration_nanos(finished_at, started_at);
        if poll.window != 0 && self.runtime.control.timing_window.load(Ordering::Acquire) == poll.window {
            without_recording(|| {
                let mut measurements = lock(&self.worker.measurements);
                if self.runtime.control.timing_window.load(Ordering::Acquire) == poll.window {
                    measurements.add(poll.window, poll.task, started_at, elapsed);
                }
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
        let mut slot = lock(&self.worker.provider);
        self.worker.current_task.store(0, Ordering::Relaxed);
        let previous = self.worker.state.swap(WorkerState::Stopped.wire_value(), Ordering::AcqRel);
        let provider = slot.take();
        drop(slot);
        drop(provider);
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
