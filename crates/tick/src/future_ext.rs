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
