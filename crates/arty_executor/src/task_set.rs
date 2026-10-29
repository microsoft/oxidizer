// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{ExecutorShared, TaskCompletion, TaskOptions};

/// Represents the set of tasks currently registered with an executor and allows new tasks
/// to be registered.
///
/// The task set acts like a handle and can be cheaply cloned and used from anywhere on the same
/// thread as the executor, including from inside the futures of tasks running on the same executor.
/// Every clone is functionally identical.
#[derive(Clone, Debug)]
pub struct TaskSet {
    shared: ExecutorShared,
}

impl TaskSet {
    pub(crate) fn new(core: &ExecutorShared) -> Self {
        Self { shared: core.clone() }
    }

    /// Registers a future and its runtime integration options as a new task.
    ///
    /// The future will be polled until it completes, after which it will be dropped, unless the
    /// executor is commanded to shut down, in which case the future may be dropped before it
    /// completes. Use [`TaskOptions::default()`] when no execution context or
    /// placement metadata is required.
    pub fn add<F, R, T>(&self, future: F, options: TaskOptions, completion: T)
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
    {
        self.shared.add_task(future, options, completion);
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
