// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared internal worker registration and state.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp};
use seismograph::recorder::runtime::{TaskId, WorkerId};
use seismograph::recorder::{is_enabled, without_recording};

use crate::snapshot::WorkerState;
use crate::{RuntimeHandle, WorkerControl, duration_nanos, lock, record_now};

/// Lightweight worker identity used when a worker submits another task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerOrigin {
    runtime: seismograph::recorder::runtime::RuntimeId,
    worker: WorkerId,
}

impl WorkerOrigin {
    pub(crate) const fn new(runtime: seismograph::recorder::runtime::RuntimeId, worker: WorkerId) -> Self {
        Self { runtime, worker }
    }

    /// Returns the process-monotonic worker identity.
    #[must_use]
    pub const fn id(&self) -> WorkerId {
        self.worker
    }

    pub(crate) fn worker_for(self, runtime: seismograph::recorder::runtime::RuntimeId) -> WorkerId {
        assert_eq!(self.runtime, runtime, "use a worker origin from the task's runtime");
        self.worker
    }
}

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerMetadata {
    pub(crate) role: WorkerRole,
    pub(crate) processor_index: Option<u32>,
}

impl WorkerMetadata {
    /// Creates worker metadata without processor affinity.
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

#[derive(Debug)]
pub(crate) struct WorkerRegistration {
    handle: WorkerHandle,
}

impl WorkerRegistration {
    pub(crate) fn new(runtime: RuntimeHandle, worker: Arc<WorkerControl>) -> Self {
        Self {
            handle: WorkerHandle { runtime, worker },
        }
    }

    pub(crate) fn id(&self) -> WorkerId {
        self.handle.id()
    }

    pub(crate) fn handle(&self) -> WorkerHandle {
        self.handle.clone()
    }

    pub(crate) fn attach_current_thread(&self) {
        self.handle.attach_current_thread();
    }
}

impl Drop for WorkerRegistration {
    fn drop(&mut self) {
        self.handle.stop();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WorkerHandle {
    runtime: RuntimeHandle,
    worker: Arc<WorkerControl>,
}

impl WorkerHandle {
    pub(crate) const fn new(runtime: RuntimeHandle, worker: Arc<WorkerControl>) -> Self {
        Self { runtime, worker }
    }

    pub(crate) fn is_observed(&self) -> bool {
        self.runtime.control.requests.load(Ordering::Relaxed) != 0 || is_enabled(EventClass::RuntimeTask)
    }

    pub(crate) fn register_task_provider(
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

    pub(crate) fn id(&self) -> WorkerId {
        self.worker.id
    }

    fn attach_current_thread(&self) {
        let thread_id = seismograph::recorder::thread_id();
        self.worker.thread_id.store(thread_id.get(), Ordering::Release);
    }

    pub(crate) fn parked(&self) {
        self.worker.state.store(WorkerState::Parked.wire_value(), Ordering::Release);
        self.record(EventKind::WorkerParked, self.id().get(), 0, 0, 0, BacktraceCapture::Never);
    }

    pub(crate) fn unparked(&self) {
        self.worker.state.store(WorkerState::Running.wire_value(), Ordering::Release);
        self.record(EventKind::WorkerUnparked, self.id().get(), 0, 0, 0, BacktraceCapture::Never);
    }

    pub(crate) fn begin_task_poll(&self, task_id: TaskId) -> u64 {
        let window = self.runtime.control.timing_window.load(Ordering::Acquire);
        self.worker.current_task.store(task_id.get(), Ordering::Release);
        window
    }

    pub(crate) fn finish_task_poll(&self, task_id: TaskId, window: u64, started_at: EventTimestamp, finished_at: EventTimestamp) {
        self.worker.current_task.store(0, Ordering::Release);
        if window != 0 && self.runtime.control.timing_window.load(Ordering::Acquire) == window {
            without_recording(|| {
                self.collect_poll_measurement(task_id, window, started_at, duration_nanos(finished_at, started_at));
            });
        }
    }

    fn collect_poll_measurement(&self, task: TaskId, window: u64, started_at: EventTimestamp, elapsed: u64) {
        let mut measurements = lock(&self.worker.measurements);
        if self.runtime.control.timing_window.load(Ordering::Acquire) == window {
            measurements.add(window, &self.runtime.control.timing_window, task, started_at, elapsed);
        }
    }

    fn stop(&self) {
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
}

#[cfg(test)]
pub(crate) struct TestTaskPoll {
    pub(crate) task: TaskId,
    pub(crate) started_at: Option<EventTimestamp>,
    pub(crate) window: u64,
    pub(crate) active: bool,
    worker: WorkerHandle,
}

#[cfg(test)]
impl WorkerHandle {
    pub(crate) fn poll_started_with(&self, task: TaskId, now: impl FnOnce() -> EventTimestamp) -> TestTaskPoll {
        let window = self.runtime.control.timing_window.load(Ordering::Acquire);
        let started_at = (window != 0 || is_enabled(EventClass::RuntimeTask)).then(now);
        self.worker.current_task.store(task.get(), Ordering::Release);
        TestTaskPoll {
            task,
            started_at,
            window,
            active: true,
            worker: self.clone(),
        }
    }

    pub(crate) fn poll_finished_with(&self, mut poll: TestTaskPoll, now: impl FnOnce() -> EventTimestamp) {
        assert_eq!(poll.worker.id(), self.id());
        poll.active = false;
        self.worker.current_task.store(0, Ordering::Release);
        let Some(started_at) = poll.started_at else { return };
        let finished_at = now();
        self.finish_task_poll(poll.task, poll.window, started_at, finished_at);
    }
}

#[cfg(test)]
impl Drop for TestTaskPoll {
    fn drop(&mut self) {
        if self.active {
            let _previous = self
                .worker
                .worker
                .current_task
                .compare_exchange(self.task.get(), 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}
