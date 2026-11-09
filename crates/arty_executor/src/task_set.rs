// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{ExecutorShared, TaskCompletion, TaskOptions};

/// A handle that registers tasks with an executor.
///
/// You can clone this handle and use each clone on the executor thread. A task
/// future can use a clone to register another task on the same executor.
#[derive(Clone, Debug)]
pub struct TaskSet {
    shared: ExecutorShared,
}

impl TaskSet {
    /// Adds a task whose description is built only by explicit inspection.
    ///
    /// The inspector is stored with the native future, without a separate task
    /// allocation. Ordinary execution cycles do not call it.
    #[cfg(feature = "inspection")]
    pub fn add_with_inspection<F, R, T, D, I>(&self, future: F, options: TaskOptions, completion: T, inspect: I)
    where
        F: Future<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
        D: 'static,
        I: Fn(&F, crate::TaskState) -> Option<D> + 'static,
    {
        self.add_with_completion_inspection(future, options, completion, move |future, _, state| inspect(future, state));
    }

    /// Adds a task whose inspector borrows its future and completion target.
    ///
    /// Store shared diagnostic metadata in the completion target to avoid a
    /// separate allocation. The inspector borrows it only during explicit
    /// inspection or observed polls on the executor thread. Ordinary cycles
    /// do not call the inspector. Terminal delivery consumes the same target
    /// through [`TaskCompletion`].
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "inspection")] {
    /// use arty_executor::{Executor, ShutdownCause, TaskCompletion, TaskOptions, TaskPanicPayload};
    ///
    /// #[derive(Debug)]
    /// struct Completion {
    ///     id: u64,
    /// }
    /// impl TaskCompletion<()> for Completion {
    ///     fn completed(self, _: ()) {}
    ///     fn panicked(self, panic: TaskPanicPayload, _: Option<ShutdownCause>) {
    ///         arty_executor::discard_task_panic_payload(panic);
    ///     }
    ///     fn cancelled(self, _: ShutdownCause) {}
    /// }
    ///
    /// let mut executor = Executor::builder().build();
    /// executor.tasks().add_with_completion_inspection(
    ///     std::future::pending::<()>(),
    ///     TaskOptions::default(),
    ///     Completion { id: 7 },
    ///     |_, completion, _| Some(completion.id),
    /// );
    /// let mut ids = Vec::new();
    /// executor.inspect(1, |id: &u64| ids.push(*id)).unwrap();
    /// assert_eq!(ids, [7]);
    /// # }
    /// ```
    #[cfg(feature = "inspection")]
    pub fn add_with_completion_inspection<F, R, T, D, I>(&self, future: F, options: TaskOptions, completion: T, inspect: I)
    where
        F: Future<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
        D: 'static,
        I: Fn(&F, &T, crate::TaskState) -> Option<D> + 'static,
    {
        self.shared
            .add_inspected_task(future, options, completion, crate::inspection::InspectWith::new(inspect));
    }

    pub(crate) fn new(core: &ExecutorShared) -> Self {
        Self { shared: core.clone() }
    }

    /// Registers a future and its runtime integration options as a new task.
    ///
    /// The executor polls the future until it completes. Executor shutdown can
    /// cancel and drop the future before completion. Use [`TaskOptions::default()`]
    /// when the task needs no observed context or placement metadata.
    pub fn add<F, R, T>(&self, future: F, options: TaskOptions, completion: T)
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
    {
        self.shared.add_task(future, options, completion);
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
