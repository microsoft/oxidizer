// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::{Clock, Delay, Timeout};

/// Adds time operations to the [`Future`] trait.
pub trait FutureExt: Future {
    /// Applies a timeout to this future.
    ///
    /// The supplied [`Clock`] controls the timeout. A controlled clock makes the
    /// timeout deterministic in tests.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{Clock, FutureExt};
    ///
    /// # async fn timeout_example(clock: &Clock) -> Result<(), Box<dyn std::error::Error>> {
    /// // Apply a timeout to a long-running future.
    /// let timeout_error = clock
    ///     .delay(Duration::from_millis(700))
    ///     .timeout(&clock, Duration::from_millis(200))
    ///     .await
    ///     .err()
    ///     .ok_or_else(|| std::io::Error::other("future unexpectedly completed"))?;
    ///
    /// assert_eq!(timeout_error.to_string(), "future timed out");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use = "futures do nothing unless awaited or polled"]
    fn timeout(self, clock: &Clock, timeout: Duration) -> Timeout<Self>
    where
        Self: Sized,
    {
        Timeout::new(self, Delay::new(clock, timeout))
    }
}

impl<T: Future> FutureExt for T {}
