// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::SimpleClock;

/// A stopwatch that measures elapsed time.
///
/// Create a `Stopwatch` from [`Clock::stopwatch()`][crate::Clock::stopwatch],
/// [`SimpleClock::stopwatch()`], or [`Stopwatch::start_new()`]. With the
/// `coarse-time` feature, [`Stopwatch::start_coarse()`] uses a lower-precision
/// source with lower overhead.
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
    source: StopwatchSource,
}

#[derive(Debug)]
enum StopwatchSource {
    Clock {
        clock: SimpleClock,
        start: Instant,
    },
    #[cfg(any(feature = "coarse-time", test))]
    Coarse {
        start: crate::coarse_time::Timestamp,
    },
}

#[cfg(all(test, any(target_os = "linux", windows), not(miri)))]
mod tests;

impl Stopwatch {
    /// Creates a stopwatch with the precision of the supplied clock.
    ///
    /// The source can be a [`Clock`][crate::Clock] or a [`SimpleClock`].
    /// A controlled source follows controlled time.
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
    /// let stopwatch = Stopwatch::start_new(clock);
    /// // Perform some operation...
    /// stopwatch.elapsed()
    /// # }
    /// ```
    #[must_use]
    pub fn start_new(source: impl AsRef<SimpleClock>) -> Self {
        let clock = source.as_ref().clone();
        let start = clock.instant();
        Self {
            source: StopwatchSource::Clock { clock, start },
        }
    }

    /// Creates a stopwatch with lower-overhead instant reads.
    ///
    /// The source can be a [`Clock`][crate::Clock] or a [`SimpleClock`]. A
    /// controlled source follows controlled time.
    ///
    /// For a system source on Linux and Windows, this constructor uses a
    /// lower-precision source. The platform defines its resolution. On Windows,
    /// the resolution is normally 10 to 16 milliseconds. Other platforms use
    /// [`Instant::now`].
    ///
    /// Use this constructor only if measurements show that [`Stopwatch::elapsed`]
    /// is a performance bottleneck. Otherwise, use [`Stopwatch::start_new`].
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "coarse-time")] {
    /// use tick::{SimpleClock, Stopwatch};
    ///
    /// let stopwatch = Stopwatch::start_coarse(SimpleClock::new_system());
    /// let elapsed = stopwatch.elapsed();
    /// # let _ = elapsed;
    /// # }
    /// ```
    #[cfg(any(feature = "coarse-time", test))]
    #[must_use]
    pub fn start_coarse(source: impl AsRef<SimpleClock>) -> Self {
        let clock = source.as_ref().clone();
        // #[gamma::skip(cond.always_false, reason = "Selecting precise system time preserves the elapsed-time contract; the benchmark owns coarse-source performance coverage.")]
        if clock.is_system() {
            return Self {
                source: StopwatchSource::Coarse {
                    start: crate::coarse_time::now(),
                },
            };
        }

        let start = clock.instant();
        Self {
            source: StopwatchSource::Clock { clock, start },
        }
    }

    /// Returns the elapsed time since the stopwatch started or last restarted.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match &self.source {
            StopwatchSource::Clock { clock, start } => clock.instant().saturating_duration_since(*start),
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { start } => crate::coarse_time::now().elapsed_since(*start),
        }
    }

    /// Returns the elapsed time and starts a new interval.
    ///
    /// This method reads the time source once. The reading ends the current
    /// interval and starts the next interval.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "test-util")]
    /// # {
    /// use std::time::Duration;
    ///
    /// use tick::{ClockControl, Stopwatch};
    ///
    /// let control = ClockControl::new();
    /// let mut stopwatch = Stopwatch::start_new(control.to_simple_clock());
    ///
    /// control.advance(Duration::from_secs(2));
    /// assert_eq!(stopwatch.restart(), Duration::from_secs(2));
    /// assert_eq!(stopwatch.elapsed(), Duration::ZERO);
    /// # }
    /// ```
    #[must_use]
    pub fn restart(&mut self) -> Duration {
        match &mut self.source {
            StopwatchSource::Clock { clock, start } => {
                let now = clock.instant();
                let elapsed = now.saturating_duration_since(*start);
                *start = now;
                elapsed
            }
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { start } => {
                let now = crate::coarse_time::now();
                let elapsed = now.elapsed_since(*start);
                // #[gamma::skip(stmt.delete_assign, reason = "The precise-clock branch has exact deterministic restart coverage; the coarse branch differs only by its intentionally unmocked platform timestamp source.")]
                *start = now;
                elapsed
            }
        }
    }
}
