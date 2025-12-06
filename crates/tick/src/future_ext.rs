// Copyright (c) Microsoft Corporation.

use std::time::Duration;

use super::{Clock, Delay, Timeout};

/// Extensions for the [`Future`] trait.
pub trait FutureExt: Future {
    /// Applies a timeout to the future.
    ///
    /// This extension uses a [`Clock`] to control the flow of time and enables
    /// easy testability.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{Clock, Delay, FutureExt};
    ///
    /// # async fn timeout_example(clock: &Clock) {
    /// // Create a long-running future
    /// let future = Delay::new(&clock, Duration::from_millis(700));
    ///
    /// // Apply a timeout to the future and await it
    /// let timeout_error = future
    ///     .timeout(Duration::from_millis(200), &clock)
    ///     .await
    ///     .unwrap_err();
    ///
    /// assert_eq!(timeout_error.to_string(), "future timed out");
    /// # }
    /// ```
    fn timeout(self, timeout: Duration, clock: &Clock) -> Timeout<Self, Delay>
    where
        Self: Sized,
    {
        Timeout::new(self, Delay::new(clock, timeout))
    }
}

impl<T> FutureExt for T where T: Future {}

#[cfg(test)]
mod tests {
    use std::task;

    use futures::FutureExt as _;

    use super::*;
    use crate::ClockControl;

    #[test]
    fn timeout_control() {
        let control = ClockControl::new()
            .auto_advance_with_max(Duration::from_secs(1), Duration::from_secs(2));

        let clock = control.to_clock();

        let future = Delay::new(&clock, Duration::from_secs(10));
        let mut future = future.timeout(Duration::from_secs(1), &clock);

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

    #[cfg(not(miri))]
    #[tokio::test]
    async fn timeout() {
        let clock = Clock::new_tokio();

        let future = async {
            Delay::new(&clock, Duration::from_secs(10)).await;
        };

        let error = future
            .timeout(Duration::from_millis(10), &clock)
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "future timed out");
    }

    #[cfg(not(miri))]
    #[tokio::test]
    async fn timeout_happy_path() {
        let clock = Clock::new_tokio();

        let future = async {
            Delay::new(&clock, Duration::from_millis(1)).await;
            10
        };

        let result = future
            .timeout(Duration::from_secs(10), &clock)
            .await
            .unwrap();

        assert_eq!(result, 10);
    }
}
