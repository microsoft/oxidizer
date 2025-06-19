// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use super::Clock;

/// A stopwatch that facilitates the measurement of elapsed time.
///
/// Instance of `Stopwatch` is created by passing [`Clock`][super::Clock] to the [`Stopwatch::with_clock`]
/// constructor.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxidizer_time::{Clock, Stopwatch};
///
/// fn measure(clock: &Clock) -> Duration {
///     let stopwatch = Stopwatch::with_clock(clock);
///     // Perform some operations ...
///     stopwatch.elapsed()
/// }
/// # let clock = Clock::with_control(&oxidizer_time::ClockControl::new());
/// # measure(&clock);
/// ```
#[derive(Debug)]
pub struct Stopwatch(StopwatchRepr);

#[derive(Debug)]
enum StopwatchRepr {
    #[cfg(not(any(feature = "fakes", test)))]
    System(Instant),
    #[cfg(any(feature = "fakes", test))]
    Clock(Clock, Instant),
}

impl Stopwatch {
    /// Creates a high-accuracy stopwatch that simplifies measurements of time.
    #[cfg_attr(
        not(any(feature = "fakes", test)),
        expect(
            unused_variables,
            reason = "intentionally not using self-references if fakes are disabled"
        )
    )]
    #[must_use]
    pub fn with_clock(clock: &Clock) -> Self {
        #[cfg(any(feature = "fakes", test))]
        let repr = StopwatchRepr::Clock(clock.clone(), clock.instant_now());

        #[cfg(not(any(feature = "fakes", test)))]
        let repr = StopwatchRepr::System(Instant::now());

        Self(repr)
    }

    /// Returns the elapsed time since the stopwatch was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match &self.0 {
            #[cfg(not(any(feature = "fakes", test)))]
            StopwatchRepr::System(start) => start.elapsed(),

            #[cfg(any(feature = "fakes", test))]
            StopwatchRepr::Clock(clock, start) => {
                clock.instant_now().saturating_duration_since(*start)
            }
        }
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
        let clock = Clock::new_dormant();
        let watch = Stopwatch::with_clock(&clock);

        sleep(Duration::from_millis(1));

        let elapsed = watch.elapsed();
        assert!(elapsed >= Duration::from_millis(1));
    }

    #[test]
    fn test_stopwatch_with_control() {
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);

        let watch = Stopwatch::with_clock(&clock);
        sleep(Duration::from_millis(1));
        assert_eq!(watch.elapsed(), Duration::ZERO);

        control.advance(Duration::from_secs(1));
        assert_eq!(watch.elapsed(), Duration::from_secs(1));
    }
}