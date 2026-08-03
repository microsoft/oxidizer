// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

use crate::{Delay, Error};

pin_project! {
    /// A future that races between an inner future and a deadline.
    ///
    /// - If the inner future completes before the deadline, the future's output is returned.
    /// - If the deadline is reached before the inner future completes, an error is returned.
    ///
    /// Values are created by [`FutureExt::timeout`](crate::FutureExt::timeout).
    #[derive(Debug)]
    #[must_use = "futures do nothing unless awaited or polled"]
    pub struct Timeout<F> {
        #[pin]
        future: F,
        #[pin]
        deadline: Delay,
    }
}

impl<F> Timeout<F> {
    pub(super) const fn new(future: F, deadline: Delay) -> Self {
        Self { future, deadline }
    }
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.future.poll(cx) {
            Poll::Ready(v) => Poll::Ready(Ok(v)),
            Poll::Pending => match this.deadline.poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(Error::timeout())),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
