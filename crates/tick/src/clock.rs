// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::task::Waker;
use std::time::{Duration, Instant, SystemTime};

use crate::state::ClockState;
use crate::timers::TimerKey;

/// Provides an abstraction for time-related operations.
///
/// Working with time is notoriously difficult to test and control. The clock enables time control in tests
/// while providing zero-cost overhead in production. When the `test-util` feature is enabled, the clock
/// provides additional functionality to control the passage of time. This makes tests faster and more reliable.
/// See the [Testing](#testing) section for more information.
///
/// The clock is used for:
///
/// - Retrieving the current absolute time in UTC.
/// - Creating [`Stopwatch`][super::Stopwatch] instances that
///   simplify time measurements and can be used as relative units of time.
/// - Creating [`PeriodicTimer`][super::PeriodicTimer] and [`Delay`][super::Delay] instances.
///
/// # Relative and absolute time
///
/// The clock provides two types of time representation:
///
/// - [`Stopwatch`][super::Stopwatch]: Represents relative time that is monotonic. Useful for measuring
///   elapsed time. Prefer relative time when a point in time does not cross process boundaries.
/// - Absolute time: Represents an absolute point in time in UTC via [`SystemTime`]. Use this when you need
///   absolute time support or need to interoperate with other crates using `SystemTime`. With the `fmt` feature
///   enabled, you can format `SystemTime` into different formats.
///
/// Absolute time is not monotonic and can be affected by system clock changes.
///
/// When possible, always prefer [`Stopwatch`][super::Stopwatch] over absolute time due to its monotonic properties.
/// For scenarios where you need absolute time, use [`system_time()`][Self::system_time].
///
/// # Clock construction
///
/// The clock requires a runtime to drive the registered timers. This crate provides built-in support
/// for Tokio via [`Clock::new_tokio`] (available with the `tokio` feature). For other async runtimes,
/// you can use types in the [`runtime`][crate::runtime] module to drive the clock.
///
/// In tests, the clock can be constructed directly using [`ClockControl`][crate::ClockControl] or via [`Clock::new_frozen`][crate::Clock::new_frozen]
/// (available with the `test-util` feature) because the passage of time is controlled manually.
///
/// See the [Testing](#testing) section for more information.
///
/// # Testing
///
/// When working with time, it's challenging to isolate time-related operations in tests. A typical example is the sleep
/// operation, which is hard to test and slows down tests. What you want to do is have complete control over the passage
/// of time that allows you to jump forward in time. This is where the clock comes into play.
///
/// The ability to jump forward in time makes tests faster, more reliable and gives you complete control over the passage of time.
/// By default, the clock does not allow you to control the passage of time. However, when the `test-util` feature is enabled,
/// this crate provides a [`ClockControl`][crate::ClockControl] type that can be used to control time.
///
/// # Cloning and shared state
///
/// Cloning a clock is inexpensive (just an `Arc` clone) and every clone shares the same underlying state,
/// including registered timers and—when the `test-util` feature is enabled—the controlled passage of time.
/// Any timers you register or time adjustments you perform through one clone are visible to every other clone
/// created from the same clock.
///
/// ```
/// use tick::Clock;
///
/// # fn use_clock(clock: &Clock) {
/// let clock_clone1 = clock.clone();
/// let clock_clone2 = clock.clone();
/// // All clones remain linked and observe the same timers and time control.
/// # }
/// ```
///
/// # Examples
///
/// ## Retrieve absolute time
///
/// ```
/// use std::time::SystemTime;
///
/// use tick::Clock;
///
/// # fn retrieve_absolute_time(clock: &Clock) {
/// // Using SystemTime for basic absolute time needs
/// let time1: SystemTime = clock.system_time();
/// let time2: SystemTime = clock.system_time();
///
/// assert!(time2 >= time1);
/// # }
/// ```
///
/// ## Measure elapsed time
///
/// ```
/// use std::time::Duration;
///
/// use tick::Clock;
///
/// # fn measure(clock: &Clock) {
/// let stopwatch = clock.stopwatch();
/// // Perform some operation...
/// let elapsed: Duration = stopwatch.elapsed();
/// # }
/// ```
///
/// ## Delay operations
///
/// ```
/// use std::time::Duration;
///
/// use tick::Clock;
///
/// # async fn delay_example(clock: &Clock) {
/// let stopwatch = clock.stopwatch();
///
/// // Delay for 10 milliseconds
/// clock.delay(Duration::from_millis(10)).await;
///
/// assert!(stopwatch.elapsed() >= Duration::from_millis(10));
/// # }
/// ```
///
/// ## Create periodic timers
///
/// ```
/// use std::time::Duration;
///
/// use futures::StreamExt;
/// use tick::{Clock, PeriodicTimer};
///
/// # async fn timer_example(clock: &Clock) {
/// let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(10));
///
/// while let Some(()) = timer.next().await {
///     // do something
///         # break;
/// }
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Clock(pub(crate) Arc<ClockState>);

impl Clock {
    /// Creates a new clock driven by the Tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a Tokio runtime context.
    #[cfg(any(feature = "tokio", test))]
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub fn new_tokio() -> Self {
        Self::new_tokio_core().0
    }

    #[cfg(any(feature = "tokio", test))]
    fn new_tokio_core() -> (Self, tokio::task::JoinHandle<()>) {
        /// How often the Tokio clock driver advances timers.
        ///
        /// A 10ms resolution balances precision with runtime overhead for the
        /// background task that drives timer advancement in Tokio.
        const TIMER_RESOLUTION: Duration = Duration::from_millis(10);

        let (clock, mut driver) = crate::runtime::InactiveClock::default().activate();

        let join_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TIMER_RESOLUTION).await;

                if driver.advance_timers(Instant::now()).is_err() {
                    break;
                }
            }
        });

        (clock, join_handle)
    }

    /// Used for testing. For this clock, timers do not advance.
    #[cfg(test)]
    pub(super) fn new_system_frozen() -> Self {
        Self(crate::state::GlobalState::System.into_clock_state())
    }

    /// Creates a new frozen clock.
    ///
    /// This is a convenience method equivalent to calling `ClockControl::new().to_clock()`.
    ///
    /// > **Note**: The returned clock will not advance time; all time and timers are frozen.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread::sleep;
    /// use std::time::Duration;
    ///
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_frozen();
    ///
    /// // The clock will always return the same timestamp and instant.
    /// let system_time = clock.system_time();
    /// let instance = clock.instant();
    ///
    /// sleep(Duration::from_micros(1));
    ///
    /// assert_eq!(system_time, clock.system_time());
    /// assert_eq!(instance, clock.instant());
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen() -> Self {
        crate::ClockControl::new().to_clock()
    }

    /// Creates a new frozen clock at the specified timestamp.
    ///
    /// This is a convenience method equivalent to calling `ClockControl::new_at(time).to_clock()`.
    ///
    /// > **Note**: The returned clock will not advance time; all time and timers are frozen at the specified timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Clock;
    ///
    /// let specific_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    /// let clock = Clock::new_frozen_at(specific_time);
    ///
    /// // The clock will always return the same timestamp and instant.
    /// let system_time = clock.system_time();
    ///
    /// assert_eq!(system_time, specific_time);
    /// assert_eq!(system_time, clock.system_time());
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen_at(time: impl Into<SystemTime>) -> Self {
        crate::ClockControl::new_at(time).to_clock()
    }

    /// Retrieves the current system time as [`SystemTime`].
    ///
    /// > **Note**: The system time is not monotonic and can be affected by system clock changes.
    /// > When the system clock changes, the current time may be older than a previously retrieved one.
    /// > For relative time measurements, use [`Stopwatch`][super::Stopwatch].
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::Clock;
    ///
    /// # fn retrieve_system_time(clock: &Clock) {
    /// let time1 = clock.system_time();
    /// let time2 = clock.system_time();
    ///
    /// assert!(time2 >= time1);
    /// # }
    /// ```
    #[must_use]
    pub fn system_time(&self) -> SystemTime {
        match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.system_time(),
            ClockState::System(_) => SystemTime::now(),
        }
    }

    /// Retrieves the current system time converted to a target type.
    ///
    /// This is a convenience method that retrieves the current [`SystemTime`] via
    /// [`system_time()`][Self::system_time] and converts it to the specified target type.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The target type that implements [`TryFrom<SystemTime>`]. Common examples include
    ///   timestamp types from external crates that can be constructed from a `SystemTime`.
    ///
    /// # Panics
    ///
    /// While this method uses [`TryFrom`] (a fallible conversion), it may panic on conversion failure.
    ///
    /// In practice, this conversion always succeeds because:
    ///
    /// - The system time returned is always within a normalized range in real environments.
    /// - Target types that implement `TryFrom<SystemTime>` typically support the full valid
    ///   range of system time values.
    ///
    /// The only theoretical failure case is in tests using manual time control (via the
    /// `test-util` feature), where time could be moved excessively far into the future,
    /// potentially exceeding the target type's representable range. This is not a concern
    /// in production.
    #[expect(
        clippy::match_wild_err_arm,
        clippy::panic,
        reason = "the panic might only occur when system time is outside of valid range which won't ever happen in real environments"
    )]
    #[must_use]
    pub fn system_time_as<T: TryFrom<SystemTime>>(&self) -> T {
        match T::try_from(self.system_time()) {
            Ok(time) => time,
            Err(_err) => panic!(
                "The SystemTime returned by the clock is always in normalized range and must be convertible to the target type.
                If the target type overflows, it indicates a problem with the target type not supporting valid system time range or
                we are in tests where the time was moved excessively into the future. Practically, in production, this conversion will
                always succeed.",
            ),
        }
    }

    /// Retrieves the current [`Instant`] time.
    ///
    /// An `Instant` represents a monotonic time point guaranteed to always increase.
    /// Unlike [`system_time`][Self::system_time], the instant is not affected by system clock
    /// changes and provides a stable reference point for measuring elapsed time.
    ///
    /// > **Note**: For time measurements, consider using [`Stopwatch`][super::Stopwatch] instead,
    /// > which provides a more convenient API for measuring elapsed time.
    ///
    /// > **Important**: When measuring elapsed time with [`Instant`], use [`Instant::duration_since`]
    /// > rather than `Instant::elapsed`. The `elapsed` method bypasses the clock and goes directly
    /// > to system time, which means it won't respect controlled time in tests or when using
    /// > `ClockControl`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::Clock;
    ///
    /// # fn retrieve_instant(clock: &Clock) {
    /// let instant1 = clock.instant();
    /// let instant2 = clock.instant();
    ///
    /// assert!(instant2 >= instant1);
    /// # }
    /// ```
    #[must_use]
    pub fn instant(&self) -> Instant {
        match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.instant(),
            ClockState::System(_) => Instant::now(),
        }
    }

    /// Creates a new [`Delay`][crate::Delay] that will complete after the specified duration.
    ///
    /// This is a convenience method that calls [`Delay::new`][crate::Delay::new].
    ///
    /// If the duration is [`Duration::ZERO`], the delay completes immediately.
    /// If the duration is [`Duration::MAX`], the delay never completes.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::Clock;
    ///
    /// # async fn delay_example(clock: &Clock) {
    /// let stopwatch = clock.stopwatch();
    ///
    /// // Delay for 10 milliseconds
    /// clock.delay(Duration::from_millis(10)).await;
    ///
    /// assert!(stopwatch.elapsed() >= Duration::from_millis(10));
    /// # }
    /// ```
    #[must_use]
    pub fn delay(&self, duration: Duration) -> crate::Delay {
        crate::Delay::new(self, duration)
    }

    /// Creates a new [`Stopwatch`][crate::Stopwatch] that starts measuring elapsed time.
    ///
    /// This is a convenience method that calls [`Stopwatch::new`][crate::Stopwatch::new].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::Clock;
    ///
    /// # async fn stopwatch_example(clock: &Clock) {
    /// let stopwatch = clock.stopwatch();
    ///
    /// // Perform some operation...
    /// clock.delay(Duration::from_millis(10)).await;
    ///
    /// assert!(stopwatch.elapsed() >= Duration::from_millis(10));
    /// # }
    /// ```
    #[must_use]
    pub fn stopwatch(&self) -> crate::Stopwatch {
        crate::Stopwatch::new(self)
    }

    pub(super) fn register_timer(&self, when: Instant, waker: Waker) -> TimerKey {
        match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.register_timer(when, waker),
            ClockState::System(timers) => timers.with_timers(|t| t.register(when, waker)),
        }
    }

    pub(super) fn unregister_timer(&self, key: TimerKey) {
        match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.unregister_timer(key),
            ClockState::System(timers) => timers.with_timers(|t| t.unregister(key)),
        }
    }

    pub(crate) fn clock_state(&self) -> &ClockState {
        self.0.as_ref()
    }
}

impl AsRef<Self> for Clock {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #![allow(clippy::arithmetic_side_effects, reason = "no need to be strict in tests")]

    use std::{fmt::Debug, thread::sleep};

    use crate::ClockControl;

    use super::*;

    static_assertions::assert_impl_all!(Clock: Debug, Send, Sync, Clone, AsRef<Clock>);

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Clock: Send, Sync, AsRef<Clock>);
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn test_now() {
        let now = std::time::SystemTime::now();

        let clock = Clock::new_system_frozen();
        let absolute = clock.system_time();
        assert!(absolute >= now);
    }

    #[test]
    fn test_now_with_control() {
        let control = ClockControl::new();
        let clock = control.to_clock();

        let now = clock.system_time();
        assert_eq!(now, control.system_time());

        () = control.advance(Duration::from_secs(10));

        assert_eq!(clock.system_time(), now.checked_add(Duration::from_secs(10)).unwrap());
    }

    #[test]
    fn test_instant_now() {
        let clock = Clock::new_system_frozen();
        let clock_instant = clock.instant();
        let system_instant = Instant::now();

        assert!(
            (system_instant.duration_since(clock_instant)) < Duration::from_secs(10),
            "the `Instant` retrieved from the clock is not the same as the system one"
        );
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn test_system_time() {
        let now = std::time::SystemTime::now();

        let clock = Clock::new_system_frozen();
        let system_time = clock.system_time();
        assert!(system_time >= now);
    }

    #[test]
    fn test_system_time_with_control() {
        let control = ClockControl::new();
        let clock = control.to_clock();

        let system_time = clock.system_time();
        assert_eq!(system_time, control.system_time());

        () = control.advance(Duration::from_secs(10));

        assert_eq!(clock.system_time(), control.system_time());
    }

    #[cfg(not(miri))] // The logic we call talks to the real OS, which Miri cannot do.
    #[tokio::test]
    async fn tokio_ensure_timers_advancing() {
        let clock = Clock::new_tokio();
        clock.delay(Duration::from_millis(15)).await;
    }

    #[cfg(not(miri))] // The logic we call talks to the real OS, which Miri cannot do.
    #[tokio::test]
    async fn tokio_ensure_future_finished_when_clock_dropped() {
        let (clock, handle) = Clock::new_tokio_core();

        clock.delay(Duration::from_millis(15)).await;

        drop(clock);

        handle.await.unwrap();
    }

    #[test]
    fn new_frozen_ok() {
        let clock = Clock::new_frozen();

        let now = clock.system_time();
        let instant = clock.instant();

        sleep(Duration::from_micros(1));

        // The frozen clock should return the same timestamp and instant on every call
        assert_eq!(now, clock.system_time());
        assert_eq!(instant, clock.instant());
    }

    #[test]
    fn new_frozen_at_ok() {
        let specific_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let clock = Clock::new_frozen_at(specific_time);

        let timestamp = clock.system_time();
        let system_time = clock.system_time();

        sleep(Duration::from_micros(1));

        // The frozen clock should return the same timestamp and system time on every call
        assert_eq!(system_time, specific_time);
        assert_eq!(timestamp, clock.system_time());
        assert_eq!(system_time, clock.system_time());
    }

    #[test]
    #[should_panic(expected = "The SystemTime returned by the clock is always in normalized range")]
    fn system_time_as_panics_on_conversion_failure() {
        /// A newtype that always fails conversion from `SystemTime`.
        struct AlwaysFailsConversion;

        impl TryFrom<SystemTime> for AlwaysFailsConversion {
            type Error = &'static str;

            fn try_from(_: SystemTime) -> Result<Self, Self::Error> {
                Err("conversion always fails")
            }
        }

        let clock = Clock::new_frozen();
        let _: AlwaysFailsConversion = clock.system_time_as();
    }

    #[test]
    fn as_ref_ok() {
        let clock = Clock::new_frozen();
        let _: &Clock = clock.as_ref();
    }
}
