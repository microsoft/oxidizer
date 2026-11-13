// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{ExecutorShared, TaskBuilder};

/// A handle that registers tasks with an executor.
///
/// You can clone this handle and use each clone on the executor thread. A task
/// future can use a clone to register another task on the same executor.
#[derive(Clone, Debug)]
pub struct TaskSet {
    shared: ExecutorShared,
}

impl TaskSet {
    pub(crate) fn new(core: &ExecutorShared) -> Self {
        Self { shared: core.clone() }
    }

    /// Configures a future for registration as a new task.
    ///
    /// Call [`TaskBuilder::register`] to submit the task with its completion target.
    /// The default configuration has no observed context or inspector and uses
    /// [`TaskPlacement::Any`](crate::TaskPlacement::Any).
    ///
    /// # Examples
    ///
    /// ```
    /// # fn submit<F>(tasks: &arty_executor::TaskSet, future: F, completion: impl arty_executor::TaskCompletion<F::Output>)
    /// # where F: IntoFuture + 'static, F::Output: 'static {
    /// tasks
    ///     .task(future)
    ///     .placement(arty_executor::TaskPlacement::SameThread)
    ///     .register(completion);
    /// # }
    /// ```
    pub fn task<F>(&self, future: F) -> TaskBuilder<'_, F>
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
    {
        TaskBuilder::new(&self.shared, future)
    }

    /// Returns whether this task set's executor is running a cycle on this thread.
    #[inline]
    #[must_use]
    pub fn is_executing(&self) -> bool {
        crate::executor_context::is_executing(self.shared.wake().thread_id())
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
