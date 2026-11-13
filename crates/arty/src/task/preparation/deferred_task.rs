// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

use pin_project::pin_project;

use crate::runtime::ShutdownState;
use crate::task::Context;

/// Carries only initialized admission state until the executor converts it.
///
/// Passing `DeferredTask` directly copies space reserved for its future before
/// that future exists. `IntoFuture` keeps those copies out of task submission.
pub(crate) struct DeferredTaskFactory<FF> {
    factory: FF,
    context: Context,
}

impl<FF> DeferredTaskFactory<FF> {
    pub(crate) fn new(factory: FF, context: Context) -> Self {
        Self { factory, context }
    }
}

impl<FF, F> IntoFuture for DeferredTaskFactory<FF>
where
    FF: FnOnce(Context) -> F,
    F: Future,
{
    type Output = F::Output;
    type IntoFuture = DeferredTask<FF, F>;

    fn into_future(self) -> Self::IntoFuture {
        DeferredTask::new(self.factory, self.context)
    }
}

#[pin_project(project = DeferredTaskProj)]
pub(crate) enum DeferredTask<FF, F> {
    Factory {
        factory: Option<(FF, Context)>,
    },
    Future {
        #[pin]
        future: F,
    },
}

impl<FF, F> DeferredTask<FF, F> {
    /// Keeps the factory until the first poll, so it runs inside the executor's panic boundary.
    pub(crate) fn new(factory: FF, cx: Context) -> Self {
        Self::Factory {
            factory: Some((factory, cx)),
        }
    }
}

impl<FF, F> Future for DeferredTask<FF, F>
where
    FF: FnOnce(Context) -> F,
    F: Future,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<Self::Output> {
        // Inspect unpinned state without reborrowing a future that can hold self-references.
        if let DeferredTaskProj::Factory { factory } = self.as_mut().project() {
            if matches!(
                factory
                    .as_ref()
                    .expect("an uninitialized deferred task retains its factory")
                    .1
                    .dispatcher()
                    .shutdown()
                    .state(),
                ShutdownState::Started
            ) {
                return Poll::Pending;
            }
            let (factory, context) = factory.take().expect("a deferred task factory is invoked exactly once");
            self.set(Self::Future { future: factory(context) });
        }
        match self.project() {
            DeferredTaskProj::Future { future } => future.poll(cx),
            DeferredTaskProj::Factory { .. } => unreachable!("the deferred factory is replaced by its future above"),
        }
    }
}
