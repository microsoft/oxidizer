// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Extension traits for telemetry recording.

use std::time::Duration;

use tick::Clock;

/// Result of a timed async operation.
#[derive(Debug, Clone, Copy)]
pub struct TimedResult<R> {
    /// The result of the operation.
    pub result: R,
    /// The duration of the operation.
    pub duration: Duration,
}

/// Extension trait for timing async operations.
pub trait ClockExt {
    /// Times an async operation and returns both the result and elapsed duration.
    fn timed_async<F, R>(&self, f: F) -> impl Future<Output = TimedResult<R>>
    where
        F: Future<Output = R>;
}

impl ClockExt for Clock {
    async fn timed_async<F, R>(&self, f: F) -> TimedResult<R>
    where
        F: Future<Output = R>,
    {
        let watch = self.stopwatch();
        let result = f.await;
        TimedResult {
            result,
            duration: watch.elapsed(),
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
}
