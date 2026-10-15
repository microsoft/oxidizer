// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker registration and worker-local runtime instrumentation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp};
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
    /// Runtime owning this worker.
    #[must_use]
    #[inline]
    pub fn runtime_id(&self) -> seismograph::recorder::runtime::RuntimeId {
        self.runtime.id()
    }

    /// Returns the current live poll-timing generation, or zero when not requested.
    #[must_use]
    #[inline]
    pub fn live_timing_generation(&self) -> u64 {
        self.runtime.control.live_timing_generation.load(Ordering::Acquire)
    }

    /// Whether a live request or subscription currently needs worker observation.
    #[must_use]
    #[inline]
    pub fn live_observation_enabled(&self) -> bool {
        self.runtime.control.live_observers.load(Ordering::Relaxed) != 0
    }

    /// Prepares compact task metadata for delivery to this worker.
    ///
    /// The pending token is stored in the runtime's existing submitted factory.
    /// Full inline poll instrumentation is constructed only after materialization.
    ///
    /// # Panics
    ///
    /// Panics if the task size cannot be encoded or process task identities are exhausted.
    #[must_use]
    #[inline]
    pub fn prepare_live_task(
        &self,
        descriptor: seismograph::recorder::runtime::TypeDescriptorId,
        parent: Option<TaskId>,
        future_size_bytes: usize,
    ) -> crate::task::PendingTask {
        crate::task::PendingTask::new(self.clone(), descriptor, parent, future_size_bytes)
    }

    pub(crate) fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }

    /// Registers a callback that routes task-data requests to this worker's native storage.
    ///
    /// The callback runs on the requester, not the worker, and must return promptly.
    /// Move the request to the worker and respond at a safe point. Pending requests
    /// are bounded to one per worker. Dropping the worker registration retires this provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker has already stopped. A rejected callback
    /// is dropped rather than retained by process-lifetime metadata.
    pub fn register_task_provider(
        &self,
        provider: impl Fn(crate::live::TaskRequest) + Send + Sync + 'static,
    ) -> Result<(), seismograph::Error> {
        let _suppression = seismograph::recorder::SuppressionGuard::enter();
        let replacement = crate::live::TaskProvider(Arc::new(provider));
        let mut slot = crate::lock(&self.worker.task_provider);
        if self.worker.state.load(Ordering::Acquire) == WorkerState::Stopped.wire_value() {
            return Err(seismograph::Error::new("cannot register a task provider on a stopped worker"));
        }
        let previous = slot.replace(replacement);
        drop(slot);
        drop(previous);
        self.runtime.control.live_inspection.store(true, Ordering::Release);
        Ok(())
    }

    /// Publishes the current task without collecting timing or recording events.
    #[inline]
    pub fn task_entered(&self, task_id: TaskId) {
        self.worker.current_task.store(task_id.get(), Ordering::Release);
    }

    /// Clears the independently readable current-task slot.
    #[inline]
    pub fn task_left(&self) {
        self.worker.current_task.store(0, Ordering::Release);
    }

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
        TaskPoll {
            task_id,
            started_at,
            session: None,
            task: None,
        }
    }

    pub(crate) fn task_poll_started_recorded(
        &self,
        task_id: TaskId,
        started_at: EventTimestamp,
        queued_since: Option<EventTimestamp>,
        session: Option<seismograph::recorder::RecordingSession>,
    ) -> TaskPoll {
        self.worker.current_task.store(task_id.get(), Ordering::Release);
        if let Some(session) = session {
            seismograph::recorder::record_in_session_classified(session, EventClass::RuntimeTask, || {
                Some(crate::runtime_record(
                    started_at,
                    self.runtime.id(),
                    Some(self.id()),
                    EventKind::TaskPollStarted,
                    task_id.get(),
                    0,
                    queued_since.map_or(0, |queued| duration_nanos(started_at, queued)),
                    if queued_since.is_some() { 2 } else { 0 },
                    BacktraceCapture::Never,
                ))
            });
        }
        TaskPoll {
            task_id,
            started_at,
            session,
            task: None,
        }
    }

    /// Finishes a task poll and updates aggregate poll duration.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the token prevents callers from finishing one poll twice"
    )]
    pub fn task_poll_finished(&self, poll: TaskPoll) {
        self.task_poll_finished_with_control(&poll, poll.task.as_deref());
    }

    /// Finishes a requested poll observation and returns its elapsed nanoseconds.
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the token prevents finishing the same poll twice"
    )]
    pub fn task_poll_finished_measured(&self, poll: TaskPoll) -> u64 {
        self.task_poll_finished_with_control(&poll, poll.task.as_deref())
    }

    pub(crate) fn task_poll_finished_with_control(&self, poll: &TaskPoll, task: Option<&TaskControl>) -> u64 {
        let finished_at = if let Some(task) = task
            && poll.session.is_some()
        {
            task.activity.poll_finished(poll.session, poll.started_at, EventTimestamp::now)
        } else {
            EventTimestamp::now()
        };
        self.task_poll_finished_at(poll, task, finished_at)
    }

    pub(crate) fn task_poll_finished_at(&self, poll: &TaskPoll, task: Option<&TaskControl>, finished_at: EventTimestamp) -> u64 {
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
        duration_nanos
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
        let mut slot = crate::lock(&self.worker.task_provider);
        self.worker.current_task.store(0, Ordering::Relaxed);
        let previous = self.worker.state.swap(WorkerState::Stopped.wire_value(), Ordering::AcqRel);
        let provider = slot.take();
        drop(slot);
        // Callback destruction can execute user code; never hold the slot mutex here.
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
