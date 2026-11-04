// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::SimpleClock;

/// A stopwatch that measures elapsed time.
///
/// Create a `Stopwatch` from [`Clock::stopwatch()`][crate::Clock::stopwatch],
/// [`SimpleClock::stopwatch()`], or [`Stopwatch::new()`].
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use tick::Clock;
///
/// # fn measure(clock: &Clock) -> Duration {
/// let stopwatch = clock.stopwatch();
/// // Perform some operation...
/// stopwatch.elapsed()
/// # }
/// ```
#[derive(Debug)]
pub struct Stopwatch {
    clock: SimpleClock,
    start: Instant,
}

impl Stopwatch {
    /// Creates a stopwatch with the precision of the supplied clock.
    ///
    /// The source can be a [`Clock`][crate::Clock] or a [`SimpleClock`].
    /// A controlled source follows controlled time. A fast source uses its
    /// lower-precision instant source.
    ///
    /// You can use [`Clock::stopwatch()`][crate::Clock::stopwatch] or
    /// [`SimpleClock::stopwatch()`] as a shorter form.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{Clock, Stopwatch};
    ///
    /// # fn measure(clock: &Clock) -> Duration {
    /// let stopwatch = Stopwatch::new(clock);
    /// // Perform some operation...
    /// stopwatch.elapsed()
    /// # }
    /// ```
    #[must_use]
    pub fn new(source: impl AsRef<SimpleClock>) -> Self {
        let clock = source.as_ref().clone();
        let start = clock.instant();
        Self { clock, start }
    }

    /// Returns the elapsed time since the stopwatch was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.clock.instant().saturating_duration_since(self.start)
    }
}
