// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::context::Transfer;

use crate::inspection::Inspector;
use crate::{ExecutorShared, TaskCompletion, TaskMetadata, TaskPlacement, TaskPoller};

/// Configures a task before registering it with an executor.
///
/// Create this builder with [`TaskSet::task`](crate::TaskSet::task). Configuration
/// does not convert or register the future. Dropping the builder drops its inputs
/// without registering a task.
#[derive(derive_more::Debug)]
#[must_use = "call register to submit the task"]
pub struct TaskBuilder<'a, F, I = (), P = ()> {
    shared: &'a ExecutorShared,
    #[debug(skip)]
    future: F,
    metadata: TaskMetadata,
    #[debug(skip)]
    inspector: I,
    #[debug(skip)]
    poll: P,
}

impl<'a, F> TaskBuilder<'a, F> {
    pub(crate) fn new(shared: &'a ExecutorShared, future: F) -> Self {
        Self {
            shared,
            future,
            metadata: TaskMetadata::default(),
            inspector: (),
            poll: (),
        }
    }
}

impl<'a, F, I, P> TaskBuilder<'a, F, I, P> {
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

    /// Sets a callback that describes the converted future during explicit inspection.
    ///
    /// The callback is stored with the future without a separate task allocation.
    /// Ordinary execution cycles do not call it. Calling this method again replaces
    /// the previous callback.
    #[cfg(feature = "inspection")]
    pub fn inspect<D, J>(self, inspect: J) -> TaskBuilder<'a, F, impl Inspector<F::IntoFuture> + use<F, I, D, J>, P>
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
            poll: self.poll,
        }
    }

    /// Sets the runtime-owned policy that polls the converted future.
    #[doc(hidden)]
    pub fn poll_with<Q>(self, poll: Q) -> TaskBuilder<'a, F, I, Q> {
        TaskBuilder {
            shared: self.shared,
            future: self.future,
            metadata: self.metadata,
            inspector: self.inspector,
            poll,
        }
    }
}

impl<F, I, P> TaskBuilder<'_, F, I, P>
where
    F: IntoFuture + 'static,
    F::Output: 'static,
    I: Inspector<F::IntoFuture>,
    P: TaskPoller<F::IntoFuture>,
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
        self.shared
            .add_task(self.future, self.metadata, self.poll, completion, self.inspector);
    }
}
