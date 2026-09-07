// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A synchronous unit of work that may be moved to a thread where blocking is allowed.
pub type BlockingTask = Box<dyn FnOnce() + Send + 'static>;

/// Runs work on runtime-owned threads where blocking is allowed.
///
/// An I/O driver may use this facility instead of creating threads of its own. Submitted work is
/// not an async task and never runs on an async worker.
///
/// The facility remains available until every driver that received it has completed shutdown, so
/// cleanup work submitted during shutdown can still run.
pub trait BlockingTaskSpawner: Send + Sync + 'static {
    /// Accepts `task` for execution and returns without waiting for it to finish.
    fn spawn(&self, task: BlockingTask);
}
