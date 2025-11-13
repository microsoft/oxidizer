// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task;

/// When polled, yields the current thread to allow a different task to execute.
#[derive(Debug, Default)]
pub struct YieldFuture {
    first_poll_completed: bool,
}

impl Future for YieldFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        if self.first_poll_completed {
            task::Poll::Ready(())
        } else {
            self.first_poll_completed = true;
            cx.waker().wake_by_ref();
            task::Poll::Pending
        }
    }
}
