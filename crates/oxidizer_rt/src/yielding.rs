// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task;

/// When polled, yields the current thread to allow a different task to execute.
///
/// This is exposed in the public API via `yield_now()` on the `*TaskContext` types.
#[derive(Debug)]
pub struct YieldFuture {
    first_poll_completed: bool,
}

impl YieldFuture {
    pub(crate) const fn new() -> Self {
        Self {
            first_poll_completed: false,
        }
    }
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

#[cfg(test)]
mod tests {
    use futures::task::noop_waker_ref;

    use super::*;

    #[test]
    fn test_yield_future() {
        let mut future = Box::pin(YieldFuture::new());
        let mut cx = task::Context::from_waker(noop_waker_ref());

        assert_eq!(future.as_mut().poll(&mut cx), task::Poll::Pending);
        assert_eq!(future.as_mut().poll(&mut cx), task::Poll::Ready(()));
    }
}