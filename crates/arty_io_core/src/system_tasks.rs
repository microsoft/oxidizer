// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

/// A synchronous unit of work that an I/O driver delegates to the runtime.
pub type SystemTask = Box<dyn FnOnce() + Send + 'static>;

/// A cloneable handle for running I/O system work on runtime-owned threads.
///
/// An I/O driver may use this facility instead of creating threads of its own. Submitted work is
/// not an async application task and never runs on an async worker. The runtime callback must
/// permit the work to block.
///
/// The facility remains available until every driver that received it has completed shutdown, so
/// cleanup work submitted during shutdown can still run.
#[derive(Clone)]
pub struct SystemTasks {
    spawn: Arc<dyn Fn(SystemTask) + Send + Sync + 'static>,
}

impl SystemTasks {
    /// Creates a system-task handle backed by a runtime callback.
    ///
    /// The callback accepts work for execution and returns without waiting for it to finish.
    #[must_use]
    pub fn new(spawn: impl Fn(SystemTask) + Send + Sync + 'static) -> Self {
        Self { spawn: Arc::new(spawn) }
    }

    /// Accepts `task` for execution and returns without waiting for it to finish.
    pub fn spawn(&self, task: impl FnOnce() + Send + 'static) {
        (self.spawn)(Box::new(task));
    }
}

impl fmt::Debug for SystemTasks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemTasks").finish_non_exhaustive()
    }
}
