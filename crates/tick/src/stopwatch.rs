// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::SimpleClock;

/// A stopwatch that facilitates the measurement of elapsed time.
///
/// An instance of `Stopwatch` is created by calling [`Clock::stopwatch()`][crate::Clock::stopwatch],
/// [`SimpleClock::stopwatch()`], or by passing any clock to the [`Stopwatch::new()`] constructor.
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
    /// Creates a stopwatch that measures elapsed time using the supplied clock's precision.
    ///
    /// The stopwatch accepts any source that can be referenced as a [`SimpleClock`], including a
    /// [`Clock`][crate::Clock] and a [`SimpleClock`]. It measures time using the source's clock, so
    /// stopwatches created from a controlled clock respect the controlled passage of time, while
    /// stopwatches created from a fast-configured clock use its lower-precision instant source.
    ///
    /// > **Note**: Consider using [`Clock::stopwatch()`][crate::Clock::stopwatch] or
    /// > [`SimpleClock::stopwatch()`] as a shortcut for creating stopwatches.
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
