// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use seismograph::recorder::runtime::{RuntimeId, WorkerId};

/// Heavy blocking worker state intended for thread-local installation.
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

    pub(crate) fn validate_runtime(&self, runtime: RuntimeId) {
        assert_eq!(self.inner.runtime.id(), runtime, "use a blocking worker from the task's runtime");
    }
}
