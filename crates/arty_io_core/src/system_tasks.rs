// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A synchronous unit of work that an I/O driver delegates to the runtime.
pub type SystemTask = Box<dyn FnOnce() + Send + 'static>;

/// Runs I/O system work on runtime-owned threads.
///
/// An I/O driver may use this facility instead of creating threads of its own. Submitted work is
/// not an async application task and never runs on an async worker. Implementations must permit
/// the work to block.
///
/// The facility remains available until every driver that received it has completed shutdown, so
/// cleanup work submitted during shutdown can still run.
pub trait SystemTaskSpawner: Send + Sync + 'static {
    /// Accepts `task` for execution and returns without waiting for it to finish.
    fn spawn(&self, task: SystemTask);
}
