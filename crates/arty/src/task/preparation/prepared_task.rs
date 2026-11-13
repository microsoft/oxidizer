// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

use arty_executor::TaskPanicPayload;
use pin_project::pin_project;

use crate::runtime::ShutdownState;
use crate::runtime::hook::TaskPolls;
use crate::task::Context;

/// Creates a task future on its selected worker.
pub(crate) struct PreparedTaskFactory<FF> {
    factory: FF,
    context: Context,
    poll_hooks: TaskPolls,
}

impl<FF> PreparedTaskFactory<FF> {
    pub(crate) fn new(factory: FF, context: Context, poll_hooks: TaskPolls) -> Self {
        Self {
            factory,
            context,
            poll_hooks,
        }
    }
}

impl<FF, F> IntoFuture for PreparedTaskFactory<FF>
where
    FF: FnOnce(Context) -> F,
    F: Future,
{
    type Output = F::Output;
    type IntoFuture = PreparedTask<FF, F>;

    fn into_future(self) -> Self::IntoFuture {
        PreparedTask::Factory {
            factory: Some((self.factory, self.context, self.poll_hooks)),
        }
    }
}

#[pin_project(project = PreparedTaskProj)]
pub(crate) enum PreparedTask<FF, F> {
    Factory {
        factory: Option<(FF, Context, TaskPolls)>,
    },
    Future {
        #[pin]
        future: F,
        poll_hooks: TaskPolls,
    },
}

pub(crate) enum Preparation {
    Prepared,
    Ready,
    Shutdown,
}

impl<FF, F> PreparedTask<FF, F>
where
    FF: FnOnce(Context) -> F,
    F: Future,
{
    pub(crate) fn prepare(mut self: Pin<&mut Self>) -> Result<Preparation, TaskPanicPayload> {
        let PreparedTaskProj::Factory { factory: slot } = self.as_mut().project() else {
            return Ok(Preparation::Ready);
        };
        let (_, context, _) = slot.as_ref().expect("an uninitialized prepared task retains its factory");
        if matches!(context.dispatcher().shutdown().state(), ShutdownState::Started) {
            return Ok(Preparation::Shutdown);
        }
        let destination = context.thread().clone();
        let (factory, context, mut poll_hooks) = slot.take().expect("a prepared task factory is invoked exactly once");
        let future = catch_unwind(AssertUnwindSafe(|| factory(context)))?;
        poll_hooks.relocate_to(&destination);
        self.set(Self::Future { future, poll_hooks });
        Ok(Preparation::Prepared)
    }

    fn poll_prepared(self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Result<Poll<F::Output>, TaskPanicPayload> {
        match self.project() {
            PreparedTaskProj::Future { future, poll_hooks } => {
                poll_hooks.begin();
                let result = catch_unwind(AssertUnwindSafe(|| future.poll(cx)));
                poll_hooks.end();
                result
            }
            PreparedTaskProj::Factory { .. } => unreachable!("the prepared factory must create its future before polling"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedTaskPoller;

impl<FF, F> arty_executor::TaskPoller<PreparedTask<FF, F>> for PreparedTaskPoller
where
    FF: FnOnce(Context) -> F + 'static,
    F: Future + 'static,
{
    fn poll(&mut self, mut future: Pin<&mut PreparedTask<FF, F>>, cx: &mut PollContext<'_>) -> Result<Poll<F::Output>, TaskPanicPayload> {
        match future.as_mut().prepare()? {
            Preparation::Prepared | Preparation::Ready => future.poll_prepared(cx),
            Preparation::Shutdown => Ok(Poll::Pending),
        }
    }
}

impl<FF, F> Future for PreparedTask<FF, F>
where
    FF: FnOnce(Context) -> F,
    F: Future,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<Self::Output> {
        match self.as_mut().prepare() {
            Ok(Preparation::Prepared | Preparation::Ready) => match self.poll_prepared(cx) {
                Ok(outcome) => outcome,
                Err(panic) => resume_unwind(panic),
            },
            Ok(Preparation::Shutdown) => Poll::Pending,
            Err(panic) => resume_unwind(panic),
        }
    }
}
