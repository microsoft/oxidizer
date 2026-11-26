// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{RuntimeId, TaskId, WorkerId};

/// Heavy asynchronous worker state intended for thread-local installation.
#[derive(Clone, Debug)]
pub struct Worker {
    inner: Arc<WorkerInner>,
}

#[derive(Debug)]
struct WorkerInner {
    runtime: crate::Runtime,
    registration: crate::worker::WorkerRegistration,
}

impl Worker {
    pub(crate) fn new(runtime: crate::Runtime, registration: crate::worker::WorkerRegistration) -> Self {
        registration.attach_current_thread();
        Self {
            inner: Arc::new(WorkerInner { runtime, registration }),
        }
    }

    /// Returns the process-monotonic worker identity.
    #[must_use]
    pub fn id(&self) -> WorkerId {
        self.inner.registration.id()
    }

    /// Returns the runtime that owns this worker.
    #[must_use]
    pub fn runtime(&self) -> &crate::Runtime {
        &self.inner.runtime
    }

    /// Returns the lightweight identity used when this worker submits a task.
    #[must_use]
    pub fn origin(&self) -> crate::WorkerOrigin {
        crate::WorkerOrigin::new(self.inner.runtime.id(), self.id())
    }

    /// Marks this worker parked.
    pub fn parked(&self) {
        self.inner.registration.handle().parked();
    }

    /// Marks this worker running.
    pub fn unparked(&self) {
        self.inner.registration.handle().unparked();
    }

    /// Returns whether the next execution cycle needs diagnostic hooks.
    #[must_use]
    pub fn is_observed(&self) -> bool {
        self.inner.registration.handle().is_observed()
    }

    /// Routes bounded task requests to this worker's native storage.
    ///
    /// # Errors
    ///
    /// Returns an error when the worker already stopped.
    pub fn register_task_provider(&self, provider: impl Fn(super::TaskRequest) + Send + Sync + 'static) -> Result<(), seismograph::Error> {
        self.inner.registration.handle().register_task_provider(provider)
    }

    pub(crate) fn validate_runtime(&self, runtime: RuntimeId) {
        assert_eq!(
            self.inner.runtime.id(),
            runtime,
            "use an asynchronous worker from the task's runtime"
        );
    }

    pub(crate) fn begin_poll(&self, task: TaskId) -> u64 {
        self.inner.registration.handle().begin_task_poll(task)
    }

    pub(crate) fn finish_poll(&self, task: TaskId, window: u64, started_at: EventTimestamp, finished_at: EventTimestamp) {
        self.inner
            .registration
            .handle()
            .finish_task_poll(task, window, started_at, finished_at);
    }
}
