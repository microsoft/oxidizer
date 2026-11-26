// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{ExecutorShared, TaskBuilder, TaskIdentity};

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

    /// Returns the identity of the task currently polled by this executor.
    ///
    /// Returns `None` outside a task poll, including after a poll unwinds.
    /// Clones of this handle share the same current identity. The executor
    /// owns this state. It does not use thread-local storage for the identity.
    #[inline]
    #[must_use]
    pub fn current(&self) -> Option<TaskIdentity> {
        self.shared.current()
    }

    /// Returns the number of live tasks before a new task is registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.task_count()
    }

    /// Returns whether the executor has no live tasks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Configures a future for registration as a new task.
    ///
    /// Call [`TaskBuilder::register`] to submit the task with its completion target.
    /// The default configuration has no inspector or task observer.
    /// The caller supplies the task's identity. Each task must have its own identity.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn submit<F>(tasks: &arty_executor::TaskSet, future: F, completion: impl arty_executor::TaskCompletion<F::Output>)
    /// # where F: IntoFuture + 'static, F::Output: 'static {
    /// let identity = seismograph_runtime::TaskIdentity::from_location(std::panic::Location::caller());
    /// tasks.task(identity, future).register(completion);
    /// # }
    /// ```
    pub fn task<F>(&self, identity: TaskIdentity, future: F) -> TaskBuilder<'_, F>
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
    {
        TaskBuilder::new(&self.shared, identity, future)
    }

    /// Returns whether this task set's executor is running a cycle on this thread.
    #[inline]
    #[must_use]
    pub fn is_executing(&self) -> bool {
        crate::executor_context::is_executing(self.shared.wake().thread_id())
    }
}
