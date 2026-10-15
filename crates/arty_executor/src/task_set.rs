// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::{Rc, Weak};

use crate::{ExecutorCore, JoinEventLake, JoinHandle, TaskOptions};

/// Represents the set of tasks currently registered with an executor and allows new tasks
/// to be registered.
///
/// The task set acts like a handle and can be cheaply cloned and used from anywhere on the same
/// thread as the executor, including from inside the futures of tasks running on the same executor.
/// Every clone is functionally identical.
#[derive(Clone, Debug)]
pub struct TaskSet {
    // We only keep a weak reference, so even if the owner of this type does something odd, we do
    // not extend the lifetime of the `Executor`, which is governed by the `Executor` owner.
    core: Weak<ExecutorCore>,

    // Retained independently so submissions after executor drop can still produce a valid
    // disconnected join handle.
    join_events: JoinEventLake,
}

impl TaskSet {
    pub(crate) fn new(core: &Rc<ExecutorCore>) -> Self {
        Self {
            core: Rc::downgrade(core),
            join_events: core.join_event_lake(),
        }
    }

    /// Registers a future and its runtime integration options as a new task.
    ///
    /// The future will be polled until it completes, after which it will be dropped, unless the
    /// executor is commanded to shut down, in which case the future may be dropped before it
    /// completes. A submission after shutdown returns a join handle that resolves to
    /// [`JoinError`](crate::JoinError). Use [`TaskOptions::default()`] when no execution context
    /// or placement metadata is required.
    pub fn add<F, R>(&self, future: F, options: TaskOptions<R>) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        let Some(core) = self.core.upgrade() else {
            return self.join_events.disconnected();
        };

        core.add_task(future, options)
    }

    /// Begins executor shutdown without borrowing executor-owned polling state.
    pub fn begin_shutdown(&self) {
        if let Some(core) = self.core.upgrade() {
            core.request_shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(TaskSet: Send, Sync);
    }
}
