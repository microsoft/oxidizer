// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::SimpleClock;

/// A stopwatch that measures elapsed time.
///
/// Create a `Stopwatch` from [`Clock::stopwatch()`][crate::Clock::stopwatch],
/// [`SimpleClock::stopwatch()`], or [`Stopwatch::new()`]. With the
/// `coarse-time` feature, [`Stopwatch::new_coarse()`] uses a lower-precision
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
    elapsed: Duration,
}

crate::thread_aware_move!(Stopwatch);

#[derive(Debug)]
enum StopwatchSource {
    Clock {
        clock: SimpleClock,
        started_at: Option<Instant>,
    },
    #[cfg(any(feature = "coarse-time", test))]
    Coarse {
        started_at: Option<crate::coarse_time::Timestamp>,
    },
}

#[cfg(all(test, any(target_os = "linux", windows), not(miri)))]
mod tests;

impl Stopwatch {
    /// Creates a stopped stopwatch with the precision of the supplied clock.
    ///
    /// The source can be a [`Clock`][crate::Clock] or a [`SimpleClock`].
    /// A controlled source follows controlled time.
    ///
    /// Call [`start()`][Self::start] to measure time. You can use
    /// [`Clock::stopwatch()`][crate::Clock::stopwatch] or
    /// [`SimpleClock::stopwatch()`] to create a started stopwatch.
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::{Clock, Stopwatch};
    ///
    /// # fn create(clock: &Clock) {
    /// let stopwatch = Stopwatch::new(clock);
    /// assert!(!stopwatch.is_running());
    /// # }
    /// ```
    #[must_use]
    pub fn new(source: impl AsRef<SimpleClock>) -> Self {
        let clock = source.as_ref().clone();
        Self {
            source: StopwatchSource::Clock { clock, started_at: None },
            elapsed: Duration::ZERO,
        }
    }

    /// Creates a stopped stopwatch with lower-overhead instant reads.
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
    /// # #[cfg(feature = "coarse-time")] {
    /// use tick::{SimpleClock, Stopwatch};
    ///
    /// let stopwatch = Stopwatch::new_coarse(SimpleClock::new_system());
    /// assert!(!stopwatch.is_running());
    /// # }
    /// ```
    #[cfg(any(feature = "coarse-time", test))]
    #[must_use]
    pub fn new_coarse(source: impl AsRef<SimpleClock>) -> Self {
        let clock = source.as_ref().clone();
        // #[gamma::skip(cond.always_false, reason = "Selecting precise system time preserves the elapsed-time contract; the benchmark owns coarse-source performance coverage.")]
        if clock.is_system() {
            return Self {
                source: StopwatchSource::Coarse { started_at: None },
                elapsed: Duration::ZERO,
            };
        }

        Self {
            source: StopwatchSource::Clock { clock, started_at: None },
            elapsed: Duration::ZERO,
        }
    }

    /// Returns whether the stopwatch is running.
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::{SimpleClock, Stopwatch};
    ///
    /// let mut stopwatch = Stopwatch::new(SimpleClock::new_system());
    /// assert!(!stopwatch.is_running());
    ///
    /// stopwatch.start();
    /// assert!(stopwatch.is_running());
    /// ```
    #[must_use]
    pub const fn is_running(&self) -> bool {
        match &self.source {
            StopwatchSource::Clock { started_at, .. } => started_at.is_some(),
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { started_at } => started_at.is_some(),
        }
    }

    /// Starts or resumes elapsed-time measurement.
    ///
    /// This method has no effect if the stopwatch is already running.
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
    /// stopwatch.start();
    /// control.advance(Duration::from_secs(2));
    /// assert_eq!(stopwatch.elapsed(), Duration::from_secs(2));
    /// # }
    /// ```
    pub fn start(&mut self) {
        match &mut self.source {
            StopwatchSource::Clock { clock, started_at } => {
                if started_at.is_none() {
                    *started_at = Some(clock.instant());
                }
            }
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { started_at } => {
                if started_at.is_none() {
                    *started_at = Some(crate::coarse_time::now());
                }
            }
        }
    }

    /// Stops elapsed-time measurement.
    ///
    /// The stopwatch retains the elapsed time. This method has no effect if the
    /// stopwatch is already stopped.
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
    /// stopwatch.start();
    ///
    /// control.advance(Duration::from_secs(2));
    /// stopwatch.stop();
    /// control.advance(Duration::from_secs(1));
    ///
    /// assert_eq!(stopwatch.elapsed(), Duration::from_secs(2));
    /// # }
    /// ```
    pub fn stop(&mut self) {
        let interval = match &mut self.source {
            StopwatchSource::Clock { clock, started_at } => started_at
                .take()
                .map_or(Duration::ZERO, |started_at| clock.instant().saturating_duration_since(started_at)),
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { started_at } => started_at
                .take()
                .map_or(Duration::ZERO, |started_at| crate::coarse_time::now().elapsed_since(started_at)),
        };
        self.elapsed = self.elapsed.saturating_add(interval);
    }

    /// Stops the stopwatch and resets the elapsed time to zero.
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
    /// stopwatch.start();
    /// control.advance(Duration::from_secs(2));
    ///
    /// stopwatch.reset();
    ///
    /// assert!(!stopwatch.is_running());
    /// assert_eq!(stopwatch.elapsed(), Duration::ZERO);
    /// # }
    /// ```
    pub fn reset(&mut self) {
        self.elapsed = Duration::ZERO;
        match &mut self.source {
            StopwatchSource::Clock { started_at, .. } => *started_at = None,
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { started_at } => *started_at = None,
        }
    }

    /// Resets the elapsed time to zero and starts the stopwatch.
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
    /// stopwatch.start();
    /// control.advance(Duration::from_secs(2));
    ///
    /// stopwatch.restart();
    ///
    /// assert!(stopwatch.is_running());
    /// assert_eq!(stopwatch.elapsed(), Duration::ZERO);
    /// # }
    /// ```
    pub fn restart(&mut self) {
        self.elapsed = Duration::ZERO;
        match &mut self.source {
            StopwatchSource::Clock { clock, started_at } => *started_at = Some(clock.instant()),
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { started_at } => *started_at = Some(crate::coarse_time::now()),
        }
    }

    /// Returns the total elapsed time.
    ///
    /// The result includes all completed intervals and the current interval if
    /// the stopwatch is running.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        let current = match &self.source {
            StopwatchSource::Clock {
                clock,
                started_at: Some(started_at),
            } => clock.instant().saturating_duration_since(*started_at),
            StopwatchSource::Clock { started_at: None, .. } => Duration::ZERO,
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse {
                started_at: Some(started_at),
            } => crate::coarse_time::now().elapsed_since(*started_at),
            #[cfg(any(feature = "coarse-time", test))]
            StopwatchSource::Coarse { started_at: None } => Duration::ZERO,
        };
        self.elapsed.saturating_add(current)
    }
}
