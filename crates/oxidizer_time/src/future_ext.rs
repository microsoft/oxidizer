// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::{Clock, Delay, Timeout};

/// Oxidizer-specific extensions for the [`Future`] trait.
pub trait FutureExt: Future {
    /// Applies a timeout to the specified future.
    ///
    /// This extension uses a [`Clock`] to control the flow of time and allows
    /// easy testability.
    ///
    /// # Example
    ///
    /// ```
    /// use oxidizer_time::{Clock, FutureExt, Delay};
    /// use std::time::Duration;
    ///
    /// async fn timeout_example(clock: &Clock) {
    ///     // Create a long-running future
    ///     let future = Delay::with_clock(&clock, Duration::from_millis(700));
    ///
    ///     // Apply a timeout to the future and await it
    ///     let timeout_error = future.timeout_with_clock(Duration::from_millis(200), &clock).await.unwrap_err();
    ///
    ///     assert_eq!(timeout_error.to_string(), "future timed out");
    /// }
    /// # fn main() {
    /// #     let control = oxidizer_time::ClockControl::new().auto_advance_with_max(Duration::from_millis(200), Duration::from_millis(500));
    /// #     let clock = Clock::with_control(&control);
    /// #     futures::executor::block_on(timeout_example(&clock));
    /// # }
    /// ```
    fn timeout_with_clock(self, timeout: Duration, clock: &Clock) -> Timeout<Self, Delay>
    where
        Self: Sized,
    {
        Timeout::new(self, Delay::with_clock(clock, timeout))
    }
}

impl<T> FutureExt for T where T: Future {}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;
    use crate::ClockControl;
    use crate::runtime::MiniRuntime;

    #[test]
    fn timeout_with_clock_control() {
        let control = ClockControl::new()
            .auto_advance_with_max(Duration::from_secs(1), Duration::from_secs(2));

        let clock = Clock::with_control(&control);

        let future = Delay::with_clock(&clock, Duration::from_secs(10));

        let timeout_error =
            block_on(future.timeout_with_clock(Duration::from_secs(1), &clock)).unwrap_err();

        assert_eq!(timeout_error.to_string(), "future timed out");
    }

    #[test]
    fn timeout_with_clock() {
        MiniRuntime::execute(async move |clock| {
            let future = async {
                Delay::with_clock(&clock, Duration::from_secs(10)).await;
            };

            let error = future
                .timeout_with_clock(Duration::from_millis(10), &clock)
                .await
                .unwrap_err();

            assert_eq!(error.to_string(), "future timed out");
        });
    }

    #[test]
    fn timeout_with_clock_happy_path() {
        MiniRuntime::execute(async move |clock| {
            let future = async {
                Delay::with_clock(&clock, Duration::from_millis(1)).await;
                10
            };

            let result = future
                .timeout_with_clock(Duration::from_secs(10), &clock)
                .await
                .unwrap();

            assert_eq!(result, 10);
        });
    }
}