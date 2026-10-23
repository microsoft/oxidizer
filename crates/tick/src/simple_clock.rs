// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Instant, SystemTime};

use crate::state::ClockState;
use crate::thread_aware_move;

/// A clock that only reads time.
///
/// Unlike [`Clock`][crate::Clock], `SimpleClock` does not register or drive
/// timers. It provides the current [`SystemTime`] and [`Instant`]. It needs no
/// runtime or driver because it has no timers.
///
/// [`SimpleClock::new_system`] creates a ready-to-use clock that reads operating
/// system time.
///
/// Both clock types use `SimpleClock` as their time-only interface:
///
/// - [`Clock`][crate::Clock] implements [`AsRef<SimpleClock>`] and exposes
///   [`Clock::simple_clock`][crate::Clock::simple_clock]. You can pass a
///   timer-capable clock where an API accepts `SimpleClock`.
/// - With the `test-util` feature, [`ClockControl::to_simple_clock`][crate::ClockControl::to_simple_clock]
///   creates a controlled `SimpleClock`. The same
///   [`ClockControl`][crate::ClockControl] drives its time.
///
/// A time-only API, such as [`Stopwatch`][crate::Stopwatch], can accept either
/// clock type.
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
    /// Creates a `SimpleClock` that reads operating system time.
    ///
    /// The clock needs no runtime or driver.
    #[must_use]
    pub fn new_system() -> Self {
        Self(TimeKind::System)
    }

    /// Selects lower-overhead [`Instant`] reads where supported.
    ///
    /// On Linux and Windows, this option uses a lower-precision source. The
    /// platform defines its resolution. On Windows, the resolution is normally
    /// 10 to 16 milliseconds. Other platforms use [`Instant::now`].
    ///
    /// The setting applies only to this clock value. A clone can use a different
    /// setting. A stopwatch keeps the setting of its source clock. This option
    /// does not affect controlled clocks.
    ///
    /// # Performance
    ///
    /// Enable fast instant reads only if measurements show that
    /// [`SimpleClock::instant`] is a performance bottleneck. Otherwise, use the
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

    /// Creates a frozen `SimpleClock`.
    ///
    /// This is equivalent to `ClockControl::new().to_simple_clock()`.
    ///
    /// The returned clock does not advance time.
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

    /// Creates a frozen `SimpleClock` at `time`.
    ///
    /// This is equivalent to `ClockControl::new_at(time).to_simple_clock()`.
    ///
    /// The returned clock stays at the specified time.
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

    /// Creates a time-only view of [`Clock`][crate::Clock] state.
    pub(crate) fn from_state(state: &ClockState) -> Self {
        match state {
            ClockState::System(_) => Self(TimeKind::System),
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => Self::from_control(control.clone()),
        }
    }

    /// Creates a controlled `SimpleClock` from an owned [`ClockControl`][crate::ClockControl].
    ///
    /// Taking ownership avoids one `Arc` clone compared with
    /// [`from_state`][Self::from_state].
    #[cfg(any(feature = "test-util", test))]
    pub(crate) fn from_control(control: crate::ClockControl) -> Self {
        Self(TimeKind::Controlled(control))
    }

    /// Returns the current system time.
    ///
    /// [`SystemTime`] is not monotonic. A system clock change can move it
    /// backward. Use [`Stopwatch`][crate::Stopwatch] to measure elapsed time.
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

    /// Returns the current system time as a target type.
    ///
    /// This method is intentionally infallible for normalized clock data. Each
    /// format supported by `tick` must accept production clock values.
    ///
    /// If conversion failure is expected, call [`Self::system_time`]. Then call
    /// the target type's [`TryFrom<SystemTime>`] implementation. Do not change
    /// this method to return `Result`.
    ///
    /// # Panics
    ///
    /// Panics if the target type cannot represent the current system time. The
    /// target can have an unsupported range. A controlled test clock can also
    /// be outside the documented target range.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use tick::SimpleClock;
    ///
    /// let clock = SimpleClock::new_system();
    /// let time = clock.system_time_as::<SystemTime>();
    /// assert!(time <= SystemTime::now());
    /// ```
    #[expect(
        clippy::match_wild_err_arm,
        clippy::panic,
        reason = "conversion failure violates the documented normalized-format contract or a controlled test moved time outside the target range"
    )]
    #[must_use]
    pub fn system_time_as<T: TryFrom<SystemTime>>(&self) -> T {
        match T::try_from(self.system_time()) {
            Ok(time) => time,
            Err(_error) => panic!(
                "system_time_as::<{}> failed: target type cannot represent the current SystemTime (or controlled time is out of range)",
                std::any::type_name::<T>()
            ),
        }
    }

    /// Returns the current [`Instant`].
    ///
    /// An [`Instant`] does not decrease and system clock changes do not affect
    /// it. Consecutive reads can be equal.
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

    /// Creates a [`Stopwatch`][crate::Stopwatch] and starts it.
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
