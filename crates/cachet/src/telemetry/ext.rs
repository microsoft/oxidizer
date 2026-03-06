// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Extension traits for telemetry recording.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use pin_project_lite::pin_project;
use tick::{Clock, Stopwatch};

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
        inner: F,
        watch: Stopwatch,
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

/// Extension trait for timing async operations.
pub trait ClockExt {
    /// Times an async operation and returns both the result and elapsed duration.
    fn timed_async<F>(&self, f: F) -> Timed<F>
    where
        F: Future;
}

impl ClockExt for Clock {
    fn timed_async<F>(&self, f: F) -> Timed<F>
    where
        F: Future,
    {
        Timed {
            inner: f,
            watch: self.stopwatch(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    #[test]
    fn clock_ext_timed_async_measures_duration() {
        block_on(async {
            let control = tick::ClockControl::new();
            let clock = control.to_clock();

            let timed = clock
                .timed_async(async {
                    control.advance(Duration::from_millis(100));
                    42
                })
                .await;

            assert_eq!(timed.result, 42);
            assert_eq!(timed.duration, Duration::from_millis(100));
        });
    }

    #[test]
    fn clock_ext_timed_async_handles_pending() {
        use std::pin::Pin;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::{Context, Poll};

        /// A future that returns Pending on the first poll, then Ready on the second.
        struct YieldOnce {
            yielded: Arc<AtomicBool>,
        }

        impl std::future::Future for YieldOnce {
            type Output = i32;
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<i32> {
                if self.yielded.swap(true, Ordering::SeqCst) {
                    Poll::Ready(99)
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }

        block_on(async {
            let control = tick::ClockControl::new();
            let clock = control.to_clock();

            let timed = clock
                .timed_async(YieldOnce {
                    yielded: Arc::new(AtomicBool::new(false)),
                })
                .await;

            assert_eq!(timed.result, 99);
        });
    }
}
