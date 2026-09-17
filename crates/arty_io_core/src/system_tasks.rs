// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

use crate::DriverError;

/// A synchronous unit of work that an I/O driver delegates to the runtime.
pub type SystemTask = Box<dyn FnOnce() + Send + 'static>;

/// A cloneable handle for running I/O system work on runtime-owned threads.
///
/// An I/O driver may use this facility instead of creating threads of its own. Submitted work is
/// not an async application task and never runs on an async worker. The runtime callback must
/// permit the work to block.
///
/// The facility remains available while drivers and pending shutdown operations need it, including
/// during registration rollback, so cleanup work submitted during draining can still run.
/// A retained handle is not itself an execution-lifetime lease. The runtime must preserve execution
/// access for live owners and transferred cleanup even after a controller's shutdown deadline.
#[derive(Clone)]
pub struct SystemTasks {
    spawn: Arc<dyn Fn(SystemTask) -> Result<(), DriverError> + Send + Sync + 'static>,
}

impl SystemTasks {
    /// Creates a system-task handle backed by a runtime callback.
    ///
    /// The callback returns `Ok(())` after accepting execution ownership, without waiting for the
    /// work to finish. Returning an error rejects the task: the callback must not execute or enqueue
    /// it, and may drop its captures. Acceptance is not a report of successful task completion.
    #[must_use]
    pub fn new(spawn: impl Fn(SystemTask) -> Result<(), DriverError> + Send + Sync + 'static) -> Self {
        Self { spawn: Arc::new(spawn) }
    }

    /// Submits `task` for execution without waiting for it to finish.
    ///
    /// `Ok(())` means accepted, not completed. On rejection no callback is promised and the task's
    /// captures may be dropped. Retaining this handle does not guarantee admission after the
    /// execution authority of the runtime has retired.
    ///
    /// # Errors
    ///
    /// Returns the submission error from the runtime when work is not accepted, preserving its
    /// classification and underlying source.
    pub fn spawn(&self, task: impl FnOnce() + Send + 'static) -> Result<(), DriverError> {
        (self.spawn)(Box::new(task))
    }
}

impl fmt::Debug for SystemTasks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemTasks").finish_non_exhaustive()
    }
}
