// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::{Rc, Weak};

use crate::{ExecutorCore, JoinHandle};

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
}

impl TaskSet {
    pub(crate) fn new(core: &Rc<ExecutorCore>) -> Self {
        Self { core: Rc::downgrade(core) }
    }

    /// Registers a future to be processed by the executor as a new task.
    ///
    /// The future will be polled until it completes, after which it will be dropped, unless the
    /// executor is commanded to shut down, in which case the future may be dropped before it
    /// completes.
    ///
    /// # Panics
    ///
    /// Panics if the executor is already shutting down. This method will never panic when
    /// called from inside an existing task on the same executor (the task is running,
    /// therefore the executor is not shutting down).
    pub fn add<F, R>(&self, future: F) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        self.core
            .upgrade()
            .expect("task set is disconnected - executor has been dropped")
            .add_task(future)
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
