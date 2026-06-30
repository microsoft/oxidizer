// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::TimeClock;

/// A stopwatch that facilitates the measurement of elapsed time.
///
/// An instance of `Stopwatch` is created by calling [`Clock::stopwatch()`][crate::Clock::stopwatch],
/// [`TimeClock::stopwatch()`], or by passing any clock to the [`Stopwatch::new()`] constructor.
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
    clock: TimeClock,
    start: Instant,
}

impl Stopwatch {
    /// Creates a high-accuracy stopwatch that measures elapsed time.
    ///
    /// The stopwatch accepts any source that can be referenced as a [`TimeClock`], including a
    /// [`Clock`][crate::Clock] and a [`TimeClock`]. It measures time using the source's clock, so
    /// stopwatches created from a controlled clock respect the controlled passage of time.
    ///
    /// > **Note**: Consider using [`Clock::stopwatch()`][crate::Clock::stopwatch] or
    /// > [`TimeClock::stopwatch()`] as a shortcut for creating stopwatches.
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
    pub fn new(source: impl AsRef<TimeClock>) -> Self {
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

impl From<Stopwatch> for Instant {
    fn from(stopwatch: Stopwatch) -> Self {
        stopwatch.start
    }
}

impl From<Stopwatch> for Duration {
    fn from(stopwatch: Stopwatch) -> Self {
        stopwatch.elapsed()
    }
}

#[cfg(test)]
mod test {
    use std::thread::sleep;

    use super::*;
    use crate::Clock;
    use crate::clock_control::ClockControl;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Stopwatch: Send, Sync);
    }

    #[test]
    fn new_accepts_any_time_source() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let time_clock = control.to_time_clock();

        // `Stopwatch::new` accepts a `Clock`, a `TimeClock`, and references to either,
        // since all of them are `AsRef<TimeClock>`.
        let _ = Stopwatch::new(&clock);
        let _ = Stopwatch::new(clock);
        let _ = Stopwatch::new(&time_clock);
        let _ = Stopwatch::new(time_clock);
    }

    #[test]
    fn test_stopwatch() {
        let clock = Clock::new_system_frozen();
        let watch = clock.stopwatch();

        sleep(Duration::from_millis(1));

        let elapsed = watch.elapsed();
        assert!(elapsed >= Duration::from_millis(1));
    }

    #[test]
    fn test_stopwatch_with_control() {
        let control = ClockControl::new();
        let clock = control.to_clock();

        let watch = clock.stopwatch();
        sleep(Duration::from_millis(1));
        assert_eq!(watch.elapsed(), Duration::ZERO);

        control.advance(Duration::from_secs(1));
        assert_eq!(watch.elapsed(), Duration::from_secs(1));
    }

    #[test]
    fn test_stopwatch_into_instance() {
        let clock = Clock::new_frozen();
        let watch = clock.stopwatch();

        let instant: Instant = watch.into();
        assert_eq!(instant, clock.instant());
    }

    #[test]
    fn test_stopwatch_into_duration() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let watch = clock.stopwatch();
        control.advance(Duration::from_secs(1));

        let duration: Duration = watch.into();
        assert_eq!(duration, Duration::from_secs(1));
    }
}
