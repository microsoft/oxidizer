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
/// - With the `test-util` feature, [`ClockControl::to_simple_clock`][crate::ClockControl::to_simple_clock]
///   creates a controlled `SimpleClock` whose time is driven by the same
///   [`ClockControl`][crate::ClockControl].
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

    /// Retrieves the current system time converted to a target type.
    ///
    /// See [`Clock::system_time_as`][crate::Clock::system_time_as] for details.
    ///
    /// # Panics
    ///
    /// Panics if the current system time cannot be represented by the target type.
    /// This can happen if the target type supports a narrower range than `SystemTime`, or in tests
    /// when controlled time is moved outside the target type's supported range.
    #[expect(
        clippy::match_wild_err_arm,
        clippy::panic,
        reason = "conversion failure indicates the chosen target type cannot represent the current SystemTime (or, in tests, controlled time was moved out of range); panicking keeps this API infallible"
    )]
    #[must_use]
    pub fn system_time_as<T: TryFrom<SystemTime>>(&self) -> T {
        match T::try_from(self.system_time()) {
            Ok(time) => time,
            Err(_err) => panic!(
                "system_time_as::<{}> failed: target type cannot represent the current SystemTime (or controlled time is out of range)",
                std::any::type_name::<T>()
            ),
        }
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
    use std::time::Duration;

    use super::*;
    use crate::ClockControl;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(SimpleClock: Send, Sync, Clone, AsRef<SimpleClock>);
    }

    #[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
    #[test]
    fn instant_advances() {
        let clock = SimpleClock::new_system();
        let first = clock.instant();
        let second = clock.instant();
        assert!(second >= first);
    }

    #[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
    #[test]
    fn configured_fast_instant_is_clone_local() {
        let precise_clock = SimpleClock::new_system();
        let fast_clock = precise_clock.clone().with_fast_instant(true);
        let precise_again = fast_clock.clone().with_fast_instant(false);

        assert!(matches!(precise_clock.0, TimeKind::System));
        assert!(matches!(fast_clock.0, TimeKind::SystemFast));
        assert!(matches!(precise_again.0, TimeKind::System));

        _ = fast_clock.system_time();
        _ = fast_clock.instant();
    }

    #[test]
    fn fast_instant_configuration_does_not_affect_controlled_clock() {
        let control = ClockControl::new();
        let clock = control.to_simple_clock().with_fast_instant(true);
        let start = clock.instant();

        control.advance(Duration::from_secs(5));

        assert_eq!(clock.instant().duration_since(start), Duration::from_secs(5));
    }

    #[test]
    fn controlled_time_is_governed_by_clock_control() {
        let control = ClockControl::new();
        let clock = control.to_simple_clock();

        let start = clock.system_time();
        control.advance(Duration::from_secs(5));

        assert_eq!(clock.system_time(), start.checked_add(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn stopwatch_from_simple_clock() {
        let control = ClockControl::new();
        let clock = control.to_simple_clock();

        let watch = clock.stopwatch();
        control.advance(Duration::from_secs(1));

        assert_eq!(watch.elapsed(), Duration::from_secs(1));
    }

    #[test]
    fn new_frozen_does_not_advance() {
        let clock = SimpleClock::new_frozen();

        let system_time = clock.system_time();
        let instant = clock.instant();

        assert_eq!(system_time, clock.system_time());
        assert_eq!(instant, clock.instant());
    }

    #[test]
    fn new_frozen_at_uses_given_time() {
        let specific = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let clock = SimpleClock::new_frozen_at(specific);

        assert_eq!(clock.system_time(), specific);
    }

    #[test]
    fn from_clock_control_is_controlled() {
        let control = ClockControl::new();
        let owned: SimpleClock = SimpleClock::from(control.clone());
        let borrowed: SimpleClock = SimpleClock::from(&control);

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

        let from_ref: SimpleClock = SimpleClock::from(&clock);
        let from_owned: SimpleClock = SimpleClock::from(clock);

        control.advance(Duration::from_secs(2));
        let expected = control.to_simple_clock().system_time();

        assert_eq!(from_ref.system_time(), expected);
        assert_eq!(from_owned.system_time(), expected);
    }
}
