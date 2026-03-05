// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::{Clock, Delay, Timeout};

/// Extensions for the [`Future`] trait.
pub trait FutureExt: Future {
    /// Applies a timeout to the future.
    ///
    /// This extension uses a [`Clock`] to control the passage of time and enables
    /// easy testability.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{Clock, FutureExt};
    ///
    /// # async fn timeout_example(clock: &Clock) {
    /// // Create a long-running future and apply a timeout
    /// let timeout_error = clock
    ///     .delay(Duration::from_millis(700))
    ///     .timeout(&clock, Duration::from_millis(200))
    ///     .await
    ///     .unwrap_err();
    ///
    /// assert_eq!(timeout_error.to_string(), "future timed out");
    /// # }
    /// ```
    fn timeout(self, clock: &Clock, timeout: Duration) -> Timeout<Self, Delay>
    where
        Self: Sized,
    {
        Timeout::new(self, Delay::new(clock, timeout))
    }
}

impl<T: Future> FutureExt for T {}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::task;

    use futures::FutureExt as _;

    use super::*;
    use crate::ClockControl;

    #[test]
    fn timeout_control() {
        let control = ClockControl::new()
            .auto_advance(Duration::from_secs(1))
            .auto_advance_limit(Duration::from_secs(2));

        let clock = control.to_clock();

        let mut future = clock.delay(Duration::from_secs(10)).timeout(&clock, Duration::from_secs(1));

        // First poll at 0 seconds - no timeout yet.
        let mut cx = task::Context::from_waker(task::Waker::noop());
        let result = future.poll_unpin(&mut cx);
        assert!(result.is_pending());

        // Second poll at 1 second - timed out.
        let result = future.poll_unpin(&mut cx);

        let task::Poll::Ready(Err(timeout_error)) = result else {
            panic!("Expected a timeout error");
        };

        assert_eq!(timeout_error.to_string(), "future timed out");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn timeout() {
        let clock = Clock::new_tokio();

        let future = async {
            clock.delay(Duration::from_secs(10)).await;
        };

        let error = future.timeout(&clock, Duration::from_millis(10)).await.unwrap_err();

        assert_eq!(error.to_string(), "future timed out");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn timeout_happy_path() {
        let clock = Clock::new_tokio();

        let future = async {
            clock.delay(Duration::from_millis(1)).await;
            10
        };

        let result = future.timeout(&clock, Duration::from_secs(10)).await.unwrap();

        assert_eq!(result, 10);
    }
}
