// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Instant, SystemTime};

use crate::state::ClockState;
use crate::thread_aware_move;

/// A simplified clock used purely for **time retrieval**.
///
/// Unlike [`Clock`][crate::Clock], a `SimpleClock` does not register or drive timers: it only
/// exposes the current [`SystemTime`] and [`Instant`]. Because it has no timers, it requires
/// **no runtime and no driver** — [`SimpleClock::new_system`] yields a ready-to-use clock backed
/// by real OS time.
///
/// `SimpleClock` is the common denominator shared by both clock kinds:
///
/// - [`Clock`][crate::Clock] implements [`AsRef<SimpleClock>`] and exposes
///   [`Clock::simple_clock`][crate::Clock::simple_clock], so a timer-capable clock can be used
///   anywhere a `SimpleClock` is expected.
/// - With the `test-util` feature, [`ClockControl::to_simple_clock`]
///   creates a controlled `SimpleClock` whose time is driven by the same
///   [`ClockControl`].
///
/// This makes APIs that only need to read time — such as [`Stopwatch`][crate::Stopwatch] —
/// seamlessly accept either kind of clock.
///
/// # Examples
///
/// ```
/// use tick::SimpleClock;
///
/// let clock = SimpleClock::new_system();
///
/// let first = clock.instant();
/// let second = clock.instant();
///
/// assert!(second >= first);
/// ```
///
/// [`ClockControl`]: https://docs.rs/tick/latest/tick/struct.ClockControl.html
/// [`ClockControl::to_simple_clock`]: https://docs.rs/tick/latest/tick/struct.ClockControl.html#method.to_simple_clock
#[derive(Debug, Clone)]
pub struct SimpleClock(TimeKind);

#[derive(Debug, Clone)]
enum TimeKind {
    /// Reads real OS time. Stateless and zero-cost.
    System,
    /// Reads lower-precision OS time with lower overhead.
    #[cfg(any(feature = "fast-instant", test))]
    SystemFast,
    /// Reads time controlled by a [`ClockControl`][crate::ClockControl].
    #[cfg(any(feature = "test-util", test))]
    Controlled(crate::ClockControl),
}

thread_aware_move!(SimpleClock);

impl SimpleClock {
    /// Creates a `SimpleClock` backed by real operating-system time.
    ///
    /// The returned clock needs no runtime or driver; it reads time directly from the OS.
    #[must_use]
    pub fn new_system() -> Self {
        Self(TimeKind::System)
    }

    /// Configures this clock to use lower-overhead [`Instant`] retrieval where supported.
    ///
    /// On Linux and Windows, enabling this option uses a lower-precision source whose resolution is
    /// platform-dependent; on Windows it is normally 10 to 16 milliseconds. On other platforms,
    /// it delegates to [`Instant::now`].
    ///
    /// The setting belongs to this clock instance. A differently configured clone can be used
    /// independently, and stopwatches created from each clock retain that clock's setting.
    /// Controlled clocks are unaffected.
    ///
    /// # Performance
    ///
    /// Only enable fast instant retrieval when instrumentation shows that the default
    /// [`SimpleClock::instant`] retrieval is a performance bottleneck. Otherwise, retain the
    /// default precise source.
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::SimpleClock;
    ///
    /// let precise_clock = SimpleClock::new_system();
    /// let fast_clock = precise_clock.clone().with_fast_instant(true);
    ///
    /// let precise = precise_clock.instant();
    /// let fast = fast_clock.instant();
    /// ```
    #[cfg(any(feature = "fast-instant", test))]
    #[must_use]
    pub fn with_fast_instant(self, enabled: bool) -> Self {
        match self.0 {
            TimeKind::System | TimeKind::SystemFast if enabled => Self(TimeKind::SystemFast),
            TimeKind::System | TimeKind::SystemFast => Self(TimeKind::System),
            #[cfg(any(feature = "test-util", test))]
            TimeKind::Controlled(control) => Self(TimeKind::Controlled(control)),
        }
    }

    #[cfg(test)]
    pub(crate) const fn uses_fast_instant(&self) -> bool {
        matches!(self.0, TimeKind::SystemFast)
    }

    /// Creates a new frozen `SimpleClock`.
    ///
    /// This is a convenience method equivalent to calling `ClockControl::new().to_simple_clock()`.
    ///
    /// > **Note**: The returned clock will not advance time; all time is frozen.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread::sleep;
    /// use std::time::Duration;
    ///
    /// use tick::SimpleClock;
    ///
    /// let clock = SimpleClock::new_frozen();
    ///
    /// // The clock will always return the same timestamp and instant.
    /// let system_time = clock.system_time();
    /// let instant = clock.instant();
    ///
    /// sleep(Duration::from_micros(1));
    ///
    /// assert_eq!(system_time, clock.system_time());
    /// assert_eq!(instant, clock.instant());
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen() -> Self {
        crate::ClockControl::new().to_simple_clock()
    }

    /// Creates a new frozen `SimpleClock` at the specified timestamp.
    ///
    /// This is a convenience method equivalent to calling `ClockControl::new_at(time).to_simple_clock()`.
    ///
    /// > **Note**: The returned clock will not advance time; all time is frozen at the specified timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::SimpleClock;
    ///
    /// let specific_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    /// let clock = SimpleClock::new_frozen_at(specific_time);
    ///
    /// assert_eq!(clock.system_time(), specific_time);
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen_at(time: impl Into<SystemTime>) -> Self {
        crate::ClockControl::new_at(time).to_simple_clock()
    }

    /// Builds the read-only time view for a [`Clock`][crate::Clock]'s state.
    pub(crate) fn from_state(state: &ClockState) -> Self {
        match state {
            ClockState::System(_) => Self(TimeKind::System),
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => Self::from_control(control.clone()),
        }
    }

    /// Builds a controlled `SimpleClock` from an owned [`ClockControl`][crate::ClockControl].
    ///
    /// Taking ownership avoids an extra `Arc` clone compared to going through
    /// [`from_state`][Self::from_state].
    #[cfg(any(feature = "test-util", test))]
    pub(crate) fn from_control(control: crate::ClockControl) -> Self {
        Self(TimeKind::Controlled(control))
    }

    /// Retrieves the current system time as [`SystemTime`].
    ///
    /// > **Note**: The system time is not monotonic and can be affected by system clock changes.
    /// > For relative time measurements, use [`Stopwatch`][crate::Stopwatch].
    #[must_use]
    pub fn system_time(&self) -> SystemTime {
        match &self.0 {
            TimeKind::System => SystemTime::now(),
            #[cfg(any(feature = "fast-instant", test))]
            TimeKind::SystemFast => SystemTime::now(),
            #[cfg(any(feature = "test-util", test))]
            TimeKind::Controlled(control) => control.system_time(),
        }
    }

    /// Retrieves the current system time and converts it to `T`.
    ///
    /// # Errors
    ///
    /// Returns the target type's conversion error when the current [`SystemTime`] is outside
    /// the target type's representable range.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use tick::SimpleClock;
    ///
    /// let clock = SimpleClock::new_system();
    /// let time = clock.try_system_time_as::<SystemTime>()?;
    /// assert!(time <= SystemTime::now());
    ///
    /// # Ok::<(), std::convert::Infallible>(())
    /// ```
    pub fn try_system_time_as<T: TryFrom<SystemTime>>(&self) -> Result<T, T::Error> {
        T::try_from(self.system_time())
    }

    /// Retrieves the current [`Instant`].
    ///
    /// An [`Instant`] is monotonically non-decreasing and unaffected by system clock changes.
    /// Consecutive reads may be equal, but later reads do not go backward.
    #[must_use]
    pub fn instant(&self) -> Instant {
        match &self.0 {
            TimeKind::System => Instant::now(),
            #[cfg(any(feature = "fast-instant", test))]
            TimeKind::SystemFast => crate::fast_instant::now(),
            #[cfg(any(feature = "test-util", test))]
            TimeKind::Controlled(control) => control.instant(),
        }
    }

    /// Creates a new [`Stopwatch`][crate::Stopwatch] that starts measuring elapsed time.
    #[must_use]
    pub fn stopwatch(&self) -> crate::Stopwatch {
        crate::Stopwatch::new(self)
    }
}

impl AsRef<Self> for SimpleClock {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
    #[test]
    fn private_time_kind_tracks_clone_local_fast_instant_configuration() {
        // The selected private `TimeKind` variant is not observable through the public API.
        let precise_clock = SimpleClock::new_system();
        let fast_clock = precise_clock.clone().with_fast_instant(true);
        let precise_again = fast_clock.clone().with_fast_instant(false);

        assert!(matches!(precise_clock.0, TimeKind::System));
        assert!(matches!(fast_clock.0, TimeKind::SystemFast));
        assert!(matches!(precise_again.0, TimeKind::System));

        _ = fast_clock.system_time();
        _ = fast_clock.instant();

        let controlled_clock = SimpleClock::new_frozen().with_fast_instant(true);
        assert!(!controlled_clock.uses_fast_instant());
    }
}
