// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Instant, SystemTime};

use crate::state::ClockState;
use crate::thread_aware_move;

/// A simplified clock used purely for **time retrieval**.
///
/// Unlike [`Clock`][crate::Clock], a `TimeClock` does not register or drive timers: it only
/// exposes the current [`SystemTime`] and [`Instant`]. Because it has no timers, it requires
/// **no runtime and no driver** — [`TimeClock::new_system`] yields a ready-to-use clock backed
/// by real OS time.
///
/// `TimeClock` is the common denominator shared by both clock kinds:
///
/// - [`Clock`][crate::Clock] implements [`AsRef<TimeClock>`] and exposes
///   [`Clock::time_clock`][crate::Clock::time_clock], so a timer-capable clock can be used
///   anywhere a `TimeClock` is expected.
/// - With the `test-util` feature, [`ClockControl::to_time_clock`][crate::ClockControl::to_time_clock]
///   creates a controlled `TimeClock` whose time is driven by the same
///   [`ClockControl`][crate::ClockControl].
///
/// This makes APIs that only need to read time — such as [`Stopwatch`][crate::Stopwatch] —
/// seamlessly accept either kind of clock.
///
/// # Examples
///
/// ```
/// use tick::TimeClock;
///
/// let clock = TimeClock::new_system();
///
/// let first = clock.instant();
/// let second = clock.instant();
///
/// assert!(second >= first);
/// ```
#[derive(Debug, Clone)]
pub struct TimeClock(TimeKind);

#[derive(Debug, Clone)]
enum TimeKind {
    /// Reads real OS time. Stateless and zero-cost.
    System,
    /// Reads time controlled by a [`ClockControl`][crate::ClockControl].
    #[cfg(any(feature = "test-util", test))]
    Controlled(crate::ClockControl),
}

thread_aware_move!(TimeClock);

impl TimeClock {
    /// Creates a `TimeClock` backed by real operating-system time.
    ///
    /// The returned clock needs no runtime or driver; it reads time directly from the OS.
    #[must_use]
    pub fn new_system() -> Self {
        Self(TimeKind::System)
    }

    /// Creates a new frozen `TimeClock`.
    ///
    /// This is a convenience method equivalent to calling `ClockControl::new().to_time_clock()`.
    ///
    /// > **Note**: The returned clock will not advance time; all time is frozen.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread::sleep;
    /// use std::time::Duration;
    ///
    /// use tick::TimeClock;
    ///
    /// let clock = TimeClock::new_frozen();
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
        crate::ClockControl::new().to_time_clock()
    }

    /// Creates a new frozen `TimeClock` at the specified timestamp.
    ///
    /// This is a convenience method equivalent to calling `ClockControl::new_at(time).to_time_clock()`.
    ///
    /// > **Note**: The returned clock will not advance time; all time is frozen at the specified timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::TimeClock;
    ///
    /// let specific_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    /// let clock = TimeClock::new_frozen_at(specific_time);
    ///
    /// assert_eq!(clock.system_time(), specific_time);
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen_at(time: impl Into<SystemTime>) -> Self {
        crate::ClockControl::new_at(time).to_time_clock()
    }

    /// Builds the read-only time view for a [`Clock`][crate::Clock]'s state.
    pub(crate) fn from_state(state: &ClockState) -> Self {
        match state {
            ClockState::System(_) => Self(TimeKind::System),
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => Self(TimeKind::Controlled(control.clone())),
        }
    }

    /// Retrieves the current system time as [`SystemTime`].
    ///
    /// > **Note**: The system time is not monotonic and can be affected by system clock changes.
    /// > For relative time measurements, use [`Stopwatch`][crate::Stopwatch].
    #[must_use]
    pub fn system_time(&self) -> SystemTime {
        match &self.0 {
            TimeKind::System => SystemTime::now(),
            #[cfg(any(feature = "test-util", test))]
            TimeKind::Controlled(control) => control.system_time(),
        }
    }

    /// Retrieves the current system time converted to a target type.
    ///
    /// See [`Clock::system_time_as`][crate::Clock::system_time_as] for details.
    ///
    /// # Panics
    ///
    /// Panics if the current system time cannot be represented by the target type. In practice
    /// this never happens in production; it can only occur in tests that move controlled time
    /// excessively far into the future.
    #[expect(
        clippy::match_wild_err_arm,
        clippy::panic,
        reason = "the panic might only occur when system time is outside of valid range which won't ever happen in real environments"
    )]
    #[must_use]
    pub fn system_time_as<T: TryFrom<SystemTime>>(&self) -> T {
        match T::try_from(self.system_time()) {
            Ok(time) => time,
            Err(_err) => {
                panic!("The SystemTime returned by the clock is always in normalized range and must be convertible to the target type.")
            }
        }
    }

    /// Retrieves the current [`Instant`].
    ///
    /// An [`Instant`] is monotonic and unaffected by system clock changes.
    #[must_use]
    pub fn instant(&self) -> Instant {
        match &self.0 {
            TimeKind::System => Instant::now(),
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

impl AsRef<Self> for TimeClock {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ClockControl;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(TimeClock: Send, Sync, Clone, AsRef<TimeClock>);
    }

    #[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
    #[test]
    fn system_time_advances() {
        let clock = TimeClock::new_system();
        let first = clock.instant();
        let second = clock.instant();
        assert!(second >= first);
    }

    #[test]
    fn controlled_time_is_governed_by_clock_control() {
        let control = ClockControl::new();
        let clock = control.to_time_clock();

        let start = clock.system_time();
        control.advance(Duration::from_secs(5));

        assert_eq!(clock.system_time(), start.checked_add(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn stopwatch_from_time_clock() {
        let control = ClockControl::new();
        let clock = control.to_time_clock();

        let watch = clock.stopwatch();
        control.advance(Duration::from_secs(1));

        assert_eq!(watch.elapsed(), Duration::from_secs(1));
    }

    #[test]
    fn new_frozen_does_not_advance() {
        let clock = TimeClock::new_frozen();

        let system_time = clock.system_time();
        let instant = clock.instant();

        assert_eq!(system_time, clock.system_time());
        assert_eq!(instant, clock.instant());
    }

    #[test]
    fn new_frozen_at_uses_given_time() {
        let specific = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let clock = TimeClock::new_frozen_at(specific);

        assert_eq!(clock.system_time(), specific);
    }

    #[test]
    fn from_clock_control_is_controlled() {
        let control = ClockControl::new();
        let owned: TimeClock = TimeClock::from(control.clone());
        let borrowed: TimeClock = TimeClock::from(&control);

        let start = owned.system_time();
        control.advance(Duration::from_secs(3));

        let expected = start.checked_add(Duration::from_secs(3)).unwrap();
        assert_eq!(owned.system_time(), expected);
        assert_eq!(borrowed.system_time(), expected);
    }

    #[test]
    fn from_clock_preserves_controlled_time() {
        let control = ClockControl::new();
        let clock = control.to_clock();

        let from_ref: TimeClock = TimeClock::from(&clock);
        let from_owned: TimeClock = TimeClock::from(clock);

        control.advance(Duration::from_secs(2));
        let expected = control.to_time_clock().system_time();

        assert_eq!(from_ref.system_time(), expected);
        assert_eq!(from_owned.system_time(), expected);
    }
}
