// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project::pin_project;

use crate::runtime::ShutdownState;
use crate::task::Builtins;

#[pin_project]
pub(crate) struct DeferredTask<FF, F> {
    factory: Option<(FF, Builtins)>,
    #[pin]
    future: Option<F>,
}

impl<FF, F> DeferredTask<FF, F> {
    pub(crate) fn new(factory: FF, builtins: Builtins) -> Self {
        Self {
            factory: Some((factory, builtins)),
            future: None,
        }
    }
}

impl<FF, F> Future for DeferredTask<FF, F>
where
    FF: FnOnce(Builtins) -> F,
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        if this.future.as_ref().get_ref().is_none() {
            if matches!(
                this.factory
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
            let (factory, builtins) = this.factory.take().expect("a deferred task factory is invoked exactly once");
            this.future.set(Some(factory(builtins)));
        }
        this.future
            .as_mut()
            .as_pin_mut()
            .expect("the deferred future is initialized above")
            .poll(cx)
    }
}
