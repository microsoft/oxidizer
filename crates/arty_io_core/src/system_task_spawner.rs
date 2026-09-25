// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

/// A blocking task submitted by an I/O driver.
pub type SystemTask = Box<dyn FnOnce() + Send + 'static>;

/// A spawner for blocking system work.
///
/// Tasks run on runtime-owned system threads, not on async workers. A task may block.
///
/// The runtime keeps the spawner available until every driver's
/// [`shutdown`](crate::Driver::shutdown) call has returned, successfully or with an error.
#[derive(Clone)]
pub struct SystemTaskSpawner {
    spawn: Arc<dyn Fn(SystemTask) + Send + Sync + 'static>,
}

impl SystemTaskSpawner {
    /// Creates a spawner that submits tasks through `spawn`.
    ///
    /// The callback must return after accepting a task, without waiting for the task to finish.
    #[must_use]
    pub fn from_fn(spawn: impl Fn(SystemTask) + Send + Sync + 'static) -> Self {
        Self { spawn: Arc::new(spawn) }
    }

    /// Submits `task` for execution.
    ///
    /// This method returns after the task is accepted, without waiting for it to finish.
    ///
    /// # Panics
    ///
    /// Panics if the submission callback supplied to [`from_fn`](Self::from_fn) panics.
    pub fn spawn(&self, task: impl FnOnce() + Send + 'static) {
        (self.spawn)(Box::new(task));
    }
}

impl fmt::Debug for SystemTaskSpawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemTaskSpawner").finish_non_exhaustive()
    }
}
