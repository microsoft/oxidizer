// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::remote::Sender as RemoteSender;
use crate::{ExecutorShared, JoinHandle, ShutdownCause, TaskOptions};

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
    /// completes. A submission after shutdown returns a join handle that resolves to
    /// [`JoinError`](crate::JoinError). Use [`TaskOptions::default()`] when no execution context
    /// or placement metadata is required.
    pub fn add<F, R>(&self, future: F, options: TaskOptions) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        self.shared.add_task(future, options)
    }

    /// Registers a task whose result is delivered to a cross-thread receiver.
    ///
    /// Arty uses this integration boundary to create a non-[`Send`] future on its destination
    /// worker while returning a `Send` result handle to the submitting thread. Keeping the
    /// concrete future at this call site lets the executor store it inline instead of behind a
    /// `Pin<Box<dyn Future>>`. Obtain `result_tx` from
    /// [`remote::Pool::channel`](crate::remote::Pool::channel).
    pub fn add_with_remote_result<F, R>(&self, future: F, options: TaskOptions, result_tx: RemoteSender<R>)
    where
        F: IntoFuture<Output = R> + 'static,
        R: Send + 'static,
    {
        self.shared.add_remote_task(future, options, result_tx);
    }

    /// Begins executor shutdown with a cause without borrowing executor-owned polling state.
    ///
    /// The first shutdown cause remains authoritative.
    pub fn begin_shutdown(&self, cause: ShutdownCause) {
        self.shared.request_shutdown(cause);
    }

    /// Returns whether the executor owner is currently processing a cycle.
    ///
    /// Runtime integrations can use this to avoid interrupting an owner that
    /// will observe newly registered tasks before it can suspend.
    #[doc(hidden)]
    #[must_use]
    pub fn owner_is_executing(&self) -> bool {
        crate::executor_context::is_executing(self.shared.wake().thread_id())
    }

    #[cfg(test)]
    pub(crate) fn task_storage_is_empty(&self) -> bool {
        self.shared.task_storage_is_empty()
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

    #[test]
    fn owner_execution_state_tracks_task_polling() {
        let mut executor = crate::Executor::builder().build();
        let tasks = executor.tasks();
        assert!(!tasks.owner_is_executing());

        let observed = tasks.clone();
        let _task = tasks.add(
            async move {
                assert!(observed.owner_is_executing());
            },
            TaskOptions::default(),
        );

        assert_eq!(executor.execute_cycle(), crate::CycleOutcome::Suspend);
        assert!(!tasks.owner_is_executing());
    }
}
