// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Extension traits for telemetry recording.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::Stopwatch;
use pin_project_lite::pin_project;

/// Result of a timed async operation.
#[derive(Debug, Clone, Copy)]
pub struct TimedResult<R> {
    /// The result of the operation.
    pub result: R,
    /// The duration of the operation.
    pub duration: Duration,
}

pin_project! {
    /// A future that times the inner future's execution.
    #[must_use = "futures do nothing unless polled"]
    pub struct Timed<F> {
        #[pin]
        pub(crate) inner: F,
        pub(crate) watch: Stopwatch,
    }
}

impl<F: Future> Future for Timed<F> {
    type Output = TimedResult<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.inner.poll(cx) {
            Poll::Ready(result) => Poll::Ready(TimedResult {
                result,
                duration: this.watch.elapsed(),
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}
