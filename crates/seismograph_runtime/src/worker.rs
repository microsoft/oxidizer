// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared internal worker registration and state.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::event::{BacktraceCapture, EventKind, EventTimestamp};
use seismograph::recorder::runtime::WorkerId;

use crate::runtime::{RuntimeHandle, WorkerControl, record_with};
use crate::snapshot::WorkerState;

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

    pub(crate) fn stop(&self) {
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

    fn record(&self, kind: EventKind, subject_id: u64, related_id: u64, value_0: u64, value_1: u64, backtrace: BacktraceCapture) {
        record_with(
            &self.runtime.control,
            EventTimestamp::now,
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
