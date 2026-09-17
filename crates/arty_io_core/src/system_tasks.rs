// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

use crate::DriverError;

/// An opaque synchronous work item that an I/O driver delegates to the runtime.
///
/// Task erasure is private: construction boxes the closure, and [`run`](Self::run) invokes it
/// indirectly once. This offload cost is separate from the allocation-free local driver owners.
pub struct SystemTask {
    action: Box<dyn FnOnce() + Send + 'static>,
}

impl SystemTask {
    /// Boxes a synchronous work item without executing it.
    #[must_use]
    pub fn new(task: impl FnOnce() + Send + 'static) -> Self {
        Self { action: Box::new(task) }
    }

    /// Consumes and executes the work item on a runtime-owned system-work thread.
    ///
    /// The runtime must permit blocking here rather than running this on an async worker.
    pub fn run(self) {
        (self.action)();
    }
}

impl fmt::Debug for SystemTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemTask").finish_non_exhaustive()
    }
}

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
/// Accepted synchronous work retains execution authority through [`SystemTask::run`], including
/// submission of follow-up work. A later external callback needs independently retained execution
/// authority; passing it only a clone of this handle does not extend the synchronous task's lifetime.
///
/// This facility keeps an `Arc`-backed erased callback, and submission constructs one boxed
/// [`SystemTask`]. Wakers, client storage, and driver-owned resources have their own costs;
/// static driver contracts are not a promise of globally allocation-free execution.
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
        (self.spawn)(SystemTask::new(task))
    }
}

impl fmt::Debug for SystemTasks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemTasks").finish_non_exhaustive()
    }
}
