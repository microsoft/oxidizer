// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::context::Transfer;

use crate::inspection::Inspector;
use crate::{ExecutorShared, TaskCompletion, TaskMetadata, TaskPlacement};

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
    metadata: TaskMetadata,
    #[debug(skip)]
    inspector: I,
}

impl<'a, F> TaskBuilder<'a, F> {
    pub(crate) fn new(shared: &'a ExecutorShared, future: F) -> Self {
        Self {
            shared,
            future,
            metadata: TaskMetadata::default(),
            inspector: (),
        }
    }
}

impl<F, I> TaskBuilder<'_, F, I> {
    /// Sets the observed context for task polling, cleanup, and lifecycle events.
    ///
    /// By default, the task has no captured context.
    pub fn context(mut self, context: Transfer) -> Self {
        self.metadata.context = Some(context);
        self
    }

    /// Classifies how the caller selected this executor.
    ///
    /// This value affects lifecycle telemetry, not scheduling or worker selection.
    /// The default is [`TaskPlacement::Any`].
    pub fn placement(mut self, placement: TaskPlacement) -> Self {
        self.metadata.placement = placement;
        self
    }
}

#[cfg(feature = "inspection")]
impl<'a, F, I> TaskBuilder<'a, F, I> {
    /// Sets a callback that describes the converted future during explicit inspection.
    ///
    /// The callback is stored with the future without a separate task allocation.
    /// Ordinary execution cycles do not call it. Calling this method again replaces
    /// the previous callback.
    pub fn inspect<D, J>(self, inspect: J) -> TaskBuilder<'a, F, impl Inspector<F::IntoFuture> + use<F, I, D, J>>
    where
        F: IntoFuture,
        D: 'static,
        J: Fn(&F::IntoFuture, crate::TaskState) -> Option<D> + 'static,
    {
        TaskBuilder {
            shared: self.shared,
            future: self.future,
            metadata: self.metadata,
            inspector: crate::inspection::InspectWith::new(inspect),
        }
    }
}

impl<F, I> TaskBuilder<'_, F, I>
where
    F: IntoFuture + 'static,
    F::Output: 'static,
    I: Inspector<F::IntoFuture>,
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
        self.shared.add_task(self.future, self.metadata, completion, self.inspector);
    }
}
