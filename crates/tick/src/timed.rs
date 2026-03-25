// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utilities for measuring the execution time of asynchronous operations.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use pin_project_lite::pin_project;

use crate::Stopwatch;

/// The result of a timed async operation, containing both the inner future's
/// output and the elapsed [`Duration`].
///
/// Produced by awaiting a [`Timed`] future, which is created via [`Clock::timed`][crate::Clock::timed].
///
/// # Examples
///
/// ```
/// use tick::{Clock, TimedResult};
///
/// # async fn example(clock: &Clock) {
/// let TimedResult { result, duration } = clock.timed(async { 42 }).await;
/// println!("Result: {}, Duration: {:?}", result, duration);
/// assert_eq!(result, 42);
/// # }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct TimedResult<R> {
    /// The output of the inner future.
    pub result: R,
    /// The elapsed duration of the operation, measured using a monotonic clock.
    pub duration: Duration,
}

pin_project! {
    /// A future that wraps an inner future and measures its execution time.
    ///
    /// When the inner future completes, `Timed` yields a [`TimedResult`] containing
    /// both the output and the elapsed [`Duration`].
    ///
    /// Created via [`Clock::timed`][crate::Clock::timed].
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
