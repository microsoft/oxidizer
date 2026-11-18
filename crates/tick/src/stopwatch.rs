// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::SimpleClock;

/// A stopwatch that measures elapsed time.
///
/// Create a `Stopwatch` from [`Clock::stopwatch()`][crate::Clock::stopwatch],
/// [`SimpleClock::stopwatch()`], or [`Stopwatch::new()`]. With the
/// `fast-stopwatch` feature, [`Stopwatch::new_fast()`] uses a lower-precision
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
    #[cfg(any(feature = "fast-stopwatch", test))]
    Fast {
        start: crate::fast_stopwatch::Timestamp,
    },
}

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
    /// let stopwatch = Stopwatch::new(clock);
    /// // Perform some operation...
    /// stopwatch.elapsed()
    /// # }
    /// ```
    #[must_use]
    pub fn new(source: impl AsRef<SimpleClock>) -> Self {
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
    /// is a performance bottleneck. Otherwise, use [`Stopwatch::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "fast-stopwatch")] {
    /// use tick::{SimpleClock, Stopwatch};
    ///
    /// let stopwatch = Stopwatch::new_fast(SimpleClock::new_system());
    /// let elapsed = stopwatch.elapsed();
    /// # let _ = elapsed;
    /// # }
    /// ```
    #[cfg(any(feature = "fast-stopwatch", test))]
    #[must_use]
    pub fn new_fast(source: impl AsRef<SimpleClock>) -> Self {
        let clock = source.as_ref().clone();
        if clock.is_system() {
            return Self {
                source: StopwatchSource::Fast {
                    start: crate::fast_stopwatch::now(),
                },
            };
        }

        let start = clock.instant();
        Self {
            source: StopwatchSource::Clock { clock, start },
        }
    }

    /// Returns the elapsed time since the stopwatch was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match &self.source {
            StopwatchSource::Clock { clock, start } => clock.instant().saturating_duration_since(*start),
            #[cfg(any(feature = "fast-stopwatch", test))]
            StopwatchSource::Fast { start } => crate::fast_stopwatch::now().elapsed_since(*start),
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
    /// let mut stopwatch = Stopwatch::new(control.to_simple_clock());
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
            #[cfg(any(feature = "fast-stopwatch", test))]
            StopwatchSource::Fast { start } => {
                let now = crate::fast_stopwatch::now();
                let elapsed = now.elapsed_since(*start);
                *start = now;
                elapsed
            }
        }
    }
}
