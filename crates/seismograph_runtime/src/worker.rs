// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker registration and worker-local runtime instrumentation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::event::{BacktraceCapture, EventKind, EventTimestamp};
use seismograph::recorder::runtime::{TaskId, TransferId, WorkerId};

use crate::snapshot::WorkerState;
use crate::task::TaskPoll;
use crate::{RuntimeHandle, TaskControl, WorkerControl, duration_nanos, next_transfer_id, record_at, record_now};

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
    /// Returns the process-monotonic worker identity.
    #[must_use]
    pub fn id(&self) -> WorkerId {
        self.worker.id
    }

    /// Associates the current Seismograph recorder thread with this worker.
    pub fn attach_current_thread(&self) {
        let thread_id = seismograph::recorder::current_thread_id();
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
        self.task_poll_started_at(task_id, EventTimestamp::now(), None)
    }

    #[inline]
    pub(crate) fn task_poll_started_at(
        &self,
        task_id: TaskId,
        started_at: EventTimestamp,
        ready_since: Option<EventTimestamp>,
    ) -> TaskPoll {
        let ready_wait_nanos = ready_since.map_or(0, |ready_since| duration_nanos(started_at, ready_since));
        self.worker.current_task.store(task_id.get(), Ordering::Release);
        self.record_at(
            started_at,
            EventKind::TaskPollStarted,
            task_id.get(),
            0,
            ready_wait_nanos,
            u64::from(ready_since.is_some()),
            BacktraceCapture::Never,
        );
        TaskPoll { task_id, started_at }
    }

    /// Finishes a task poll and updates aggregate poll duration.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the token prevents callers from finishing one poll twice"
    )]
    pub fn task_poll_finished(&self, poll: TaskPoll) {
        self.task_poll_finished_with_control(&poll, None);
    }

    pub(crate) fn task_poll_finished_with_control(&self, poll: &TaskPoll, task: Option<&TaskControl>) {
        let finished_at = EventTimestamp::now();
        let duration_nanos = duration_nanos(finished_at, poll.started_at);
        self.runtime.control.counters.poll_count.fetch_add(1, Ordering::Relaxed);
        self.runtime
            .control
            .counters
            .poll_duration_nanos
            .fetch_add(duration_nanos, Ordering::Relaxed);
        if let Some(task) = task {
            task.poll_count.fetch_add(1, Ordering::Relaxed);
            task.poll_duration_nanos.fetch_add(duration_nanos, Ordering::Relaxed);
            task.max_poll_duration_nanos.fetch_max(duration_nanos, Ordering::Relaxed);
            task.last_poll_finished_at.store(finished_at.ticks().max(1), Ordering::Release);
        }
        self.worker.current_task.store(0, Ordering::Release);
        self.record_at(
            finished_at,
            EventKind::TaskPollFinished,
            poll.task_id.get(),
            0,
            duration_nanos,
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

    fn stop(&self) {
        self.worker.current_task.store(0, Ordering::Relaxed);
        let previous = self.worker.state.swap(WorkerState::Stopped.wire_value(), Ordering::AcqRel);
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
