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
    ///
    /// Returns `false` after that executor is dropped, even if a replacement runs
    /// on the same thread.
    #[must_use]
    pub fn is_executing(&self) -> bool {
        crate::executor_context::is_executing(self.shared.wake().executor_id())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::{Executor, ShutdownCause, TaskPanicPayload, discard_task_panic_payload};

    #[derive(Debug)]
    struct DiscardCompletion;

    impl TaskCompletion<()> for DiscardCompletion {
        fn completed(self, (): ()) {}

        fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
            discard_task_panic_payload(panic);
        }

        fn cancelled(self, _cause: ShutdownCause) {}
    }

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(TaskSet: Send, Sync);
    }

    #[test]
    fn execution_state_follows_the_owner_cycle() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();
        let observed = Rc::new(Cell::new(false));
        let task_observed = Rc::clone(&observed);
        let task_tasks = tasks.clone();
        assert!(!tasks.is_executing());
        tasks.add(
            async move {
                task_observed.set(task_tasks.is_executing());
            },
            TaskOptions::default(),
            DiscardCompletion,
        );

        _ = executor.execute_cycle();

        assert!(observed.get());
        assert!(!tasks.is_executing());
    }

    #[test]
    fn retained_task_set_does_not_observe_a_replacement_executor_cycle() {
        let first = Executor::builder().build();
        let retained = first.tasks();
        drop(first);

        let mut replacement = Executor::builder().build();
        let replacement_tasks = replacement.tasks();
        let observed = Rc::new(Cell::new((false, false)));
        let task_observed = Rc::clone(&observed);
        let task_retained = retained.clone();
        let task_replacement = replacement_tasks.clone();
        replacement_tasks.add(
            async move {
                task_observed.set((task_retained.is_executing(), task_replacement.is_executing()));
            },
            TaskOptions::default(),
            DiscardCompletion,
        );

        _ = replacement.execute_cycle();

        assert_eq!(observed.get(), (false, true));
    }
}
