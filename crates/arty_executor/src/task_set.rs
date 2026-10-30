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
