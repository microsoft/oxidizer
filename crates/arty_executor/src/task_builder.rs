// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{ExecutorShared, TaskCompletion, TaskInstrumentation};

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
    instrumentation: I,
}

impl<'a, F> TaskBuilder<'a, F> {
    pub(crate) fn new(shared: &'a ExecutorShared, future: F) -> Self {
        Self {
            shared,
            future,
            instrumentation: (),
        }
    }
}

impl<'a, F, I> TaskBuilder<'a, F, I> {
    /// Adds a callback that describes the converted future during inspection.
    pub fn inspect<D, J>(self, inspect: J) -> TaskBuilder<'a, F, impl TaskInstrumentation + use<F, I, D, J>>
    where
        I: TaskInstrumentation,
        D: 'static,
        J: Fn(crate::TaskState) -> Option<D> + 'static,
    {
        TaskBuilder {
            shared: self.shared,
            future: self.future,
            instrumentation: crate::inspection::InspectWith::new(self.instrumentation, inspect),
        }
    }

    /// Sets the runtime-owned behavior attached to this task.
    pub fn instrument<Q>(self, instrumentation: Q) -> TaskBuilder<'a, F, Q>
    where
        Q: TaskInstrumentation,
    {
        TaskBuilder {
            shared: self.shared,
            future: self.future,
            instrumentation,
        }
    }
}

impl<F, I> TaskBuilder<'_, F, I>
where
    F: IntoFuture + 'static,
    F::Output: 'static,
    I: TaskInstrumentation,
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
        self.shared.add_task(self.future, self.instrumentation, completion);
    }
}
