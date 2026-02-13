// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use super::Clock;

/// A stopwatch that facilitates the measurement of elapsed time.
///
/// An instance of `Stopwatch` is created by calling [`Clock::stopwatch()`] or by passing
/// a [`Clock`] to the [`Stopwatch::new()`] constructor.
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
pub struct Stopwatch(StopwatchRepr);

#[derive(Debug)]
enum StopwatchRepr {
    #[cfg(not(any(feature = "test-util", test)))]
    System(Instant),
    #[cfg(any(feature = "test-util", test))]
    Clock(Clock, Instant),
}

impl Stopwatch {
    /// Creates a high-accuracy stopwatch that measures elapsed time.
    ///
    /// > **Note**: Consider using [`Clock::stopwatch()`] as a shortcut for creating stopwatches.
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
    #[cfg_attr(
        not(any(feature = "test-util", test)),
        expect(unused_variables, reason = "intentionally not using self-references if test-util is disabled")
    )]
    #[must_use]
    pub fn new(clock: &Clock) -> Self {
        #[cfg(any(feature = "test-util", test))]
        let repr = StopwatchRepr::Clock(clock.clone(), clock.instant());

        #[cfg(not(any(feature = "test-util", test)))]
        let repr = StopwatchRepr::System(Instant::now());

        Self(repr)
    }

    /// Returns the elapsed time since the stopwatch was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match &self.0 {
            #[cfg(not(any(feature = "test-util", test)))]
            StopwatchRepr::System(start) => start.elapsed(),

            #[cfg(any(feature = "test-util", test))]
            StopwatchRepr::Clock(clock, start) => clock.instant().saturating_duration_since(*start),
        }
    }
}

impl From<Stopwatch> for Instant {
    fn from(stopwatch: Stopwatch) -> Self {
        match stopwatch.0 {
            #[cfg(not(any(feature = "test-util", test)))]
            StopwatchRepr::System(start) => start,

            #[cfg(any(feature = "test-util", test))]
            StopwatchRepr::Clock(_, start) => start,
        }
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
    use crate::clock_control::ClockControl;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Stopwatch: Send, Sync);
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
