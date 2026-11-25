// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{ExecutorShared, TaskCompletion, TaskMetadata, TaskObserver};

/// Configures a task before registering it with an executor.
///
/// Create this builder with [`TaskSet::task`](crate::TaskSet::task). Configuration
/// does not convert or register the future. Dropping the builder drops its inputs
/// without registering a task.
#[derive(derive_more::Debug)]
#[must_use = "call register to submit the task"]
pub struct TaskBuilder<'a, F, I = ()> {
    shared: &'a ExecutorShared,
    #[debug(skip)]
    future: F,
    #[debug(skip)]
    observer: I,
    monitored: Option<TaskMetadata>,
}

impl<'a, F> TaskBuilder<'a, F> {
    pub(crate) fn new(shared: &'a ExecutorShared, future: F) -> Self {
        Self {
            shared,
            future,
            observer: (),
            monitored: None,
        }
    }
}

impl<'a, F, I> TaskBuilder<'a, F, I> {
    /// Sets the observer for this task's top-level polls.
    pub fn observe<Q>(self, observer: Q) -> TaskBuilder<'a, F, Q>
    where
        Q: TaskObserver,
    {
        TaskBuilder {
            shared: self.shared,
            future: self.future,
            observer,
            monitored: self.monitored,
        }
    }

    /// Adds concrete native monitoring metadata.
    pub fn monitor(mut self, monitored: Option<TaskMetadata>) -> Self {
        self.monitored = monitored;
        self
    }
}

impl<F, I> TaskBuilder<'_, F, I>
where
    F: IntoFuture + 'static,
    F::Output: 'static,
    I: TaskObserver,
{
    /// Registers the task and its completion target.
    ///
    /// The executor polls the future until it completes. Shutdown can cancel and
    /// drop the future before completion. If shutdown already started, registration
    /// reports cancellation without converting the future.
    ///
    /// Registration does not wake the executor owner. The caller must arrange an
    /// [`execute_cycle`](crate::Executor::execute_cycle) after registration.
    pub fn register(self, completion: impl TaskCompletion<F::Output>) {
        self.shared.add_task(self.future, self.observer, self.monitored, completion);
    }
}
