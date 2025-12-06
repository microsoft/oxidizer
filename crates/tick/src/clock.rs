// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(feature = "tokio", test))]
use std::sync::Arc;
use std::task::Waker;
use std::time::{Instant, SystemTime};

use super::TimerKey;
#[cfg(any(feature = "test-util", test))]
use super::clock_control::ClockControl;
use crate::state::ClockState;

/// Provides an abstraction for time-related operations.
///
/// Working with time is notoriously difficult to test and control. The clock enables time control in tests
/// while providing near-zero overhead in production. When running with the `test-util` feature enabled, the clock
/// provides additional functionality to control the flow of time. This makes tests faster and more reliable.
/// See the [Testing](#testing) section for more information.
///
/// The clock is used for:
///
/// - Retrieving the current absolute time in UTC.
/// - Creation of [`Stopwatch`][super::Stopwatch] that
///   simplifies time measurements and can be used as a relative unit of time.
/// - Creating [`PeriodicTimer`][super::PeriodicTimer] and [`Delay`][super::Delay] instances.
///
/// # Relative and absolute time
///
/// The clock provides two types of time representation:
///
/// - [`Stopwatch`][super::Stopwatch]: Representation of relative time that is monotonic. This is useful for measuring
///   elapsed time. The use of relative time is recommended when a point in time does not cross process boundaries.
/// - Absolute time: Represents an absolute point in time in UTC. The clock provides absolute time through two types:
///   - [`SystemTime`]: The standard library type for absolute time. Use this when you need
///     basic absolute time support or need to interoperate with other crates using `SystemTime`.
///   - [`Timestamp`][crate::Timestamp] (optional, requires `timestamp` feature): An enhanced absolute time type that provides additional
///     formatting and parsing capabilities, serialization support, and the ability to send time information across
///     process boundaries. It can be converted to and from `SystemTime` as needed.
///
/// Both absolute time representations are not monotonic and can be affected by system clock changes.
///
/// When possible, always prefer [`Stopwatch`][super::Stopwatch] over absolute time due to its monotonic properties.
/// For scenarios where you need absolute time, use [`system_time()`][Self::system_time] for basic needs, or
/// [`timestamp()`][Self::timestamp] when you need formatting, serialization, or cross-process capabilities.
///
/// # Clock construction
///
/// The clock requires a runtime to drive the registered timers. For this reason, clock construction is non-trivial
/// and clock access is provided by the runtime.
///
/// In production, the clock is typically obtained from your async runtime. Different runtimes
/// provide different mechanisms for clock access. Consult your runtime's documentation for details.
///
/// In tests, the clock can be constructed directly using `new_frozen()` or
/// `new_frozen_at()` (available with the `test-util` feature) because the flow of time is controlled manually.
/// See the [Testing](#testing) section for more information.
///
/// # Testing
///
/// When working with time, it's challenging to isolate time-related operations in tests. A typical example is the sleep
/// operation, which is hard to test and slows down tests. What you want to do is have complete control over the flow
/// of time that allows you to jump forward in time. This is where the clock comes into play.
///
/// The ability to jump forward in time makes tests faster, more reliable and gives you complete control over the flow of time.
/// By default, the clock does not allow you to control the flow of time. However, when the `test-util` feature is enabled,
/// this crate provides a `ClockControl` type that can be used to control time.
///
/// # State sharing between clocks
///
/// Multiple clock instances can be linked together and share state. In production, cloned clocks share
/// registered timers. In tests, cloned clocks additionally share the flow of time, allowing coordinated
/// time control across all instances.
///
/// To ensure state sharing between clocks, clone the clock. The cloning operation preserves the shared state
/// between individual clocks. The clone operation is inexpensive in both production and test scenarios.
///
/// ```
/// use tick::Clock;
///
/// # fn use_clock(clock: &Clock) {
/// // Cloned clocks; all these instances are linked
/// // together and share the same state.
/// let clock_clone1 = clock.clone();
/// let clock_clone2 = clock.clone();
/// # }
/// ```
///
/// # Examples
///
/// ### Retrieve absolute time
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
/// With the `timestamp` feature enabled:
///
/// ```
/// use tick::Clock;
///
/// # fn retrieve_timestamp(clock: &Clock) {
/// // Using Timestamp for formatting, serialization, and cross-process scenarios
/// let timestamp1 = clock.timestamp();
/// let timestamp2 = clock.timestamp();
///
/// assert!(timestamp2 >= timestamp1);
/// # }
/// ```
///
/// ### Measure elapsed time
///
/// ```
/// use std::time::Duration;
///
/// use tick::{Clock, Stopwatch};
///
/// # fn measure(clock: &Clock) {
/// let stopwatch = Stopwatch::new(&clock);
/// // Perform some operation...
/// let elapsed: Duration = stopwatch.elapsed();
/// # }
/// ```
///
/// ### Delay operations
///
/// ```
/// use std::time::Duration;
///
/// use tick::{Clock, Delay, Stopwatch};
///
/// # async fn delay_example(clock: &Clock) {
/// let stopwatch = Stopwatch::new(&clock);
///
/// // Delay for 10 millis
/// Delay::new(&clock, Duration::from_millis(10)).await;
///
/// assert!(stopwatch.elapsed() >= Duration::from_millis(10));
/// # }
/// ```
///
/// ### Create periodic timers
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
pub struct Clock(ClockInner);

impl Clock {
    /// Creates a new clock driven by the Tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a Tokio runtime context.
    #[cfg(any(feature = "tokio", test))]
    #[must_use]
    pub fn new_tokio() -> Self {
        Self::tokio_core().0
    }

    #[cfg(any(feature = "tokio", test))]
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    fn tokio_core() -> (Self, tokio::task::JoinHandle<()>) {
        use std::time::Duration;

        use crate::runtime::InactiveClock;

        const TIMER_RESOLUTION: Duration = Duration::from_millis(10);

        let (state, driver) = InactiveClock::default().activate_with_state();
        let tokio_state = TokioClockState::new(state);

        // Spawn a task that advances the timers.
        let cancelation = Arc::clone(&tokio_state.cancellation);
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TIMER_RESOLUTION).await;

                // Stop the loop when there are no more timers and the clock is gone
                //
                // Cancellation flag:
                // - Each `TokioClockState` holds a clone of the cancellation token
                // - This background task also holds one clone
                // - When all Clock instances are dropped, only one instance of the cancellation token remains,
                //   which is this background task
                // - When there are no timers and no clocks left, it's a signal to drop this routine
                if driver.advance_timers(Instant::now()).is_none() && Arc::strong_count(&cancelation) == 1 {
                    break;
                }
            }
        });

        (Self(ClockInner::Tokio(tokio_state)), join_handle)
    }

    /// Used for testing. For this clock, timers do not advance.
    #[cfg(test)]
    pub(super) fn with_frozen_timers() -> Self {
        Self::with_state(crate::state::GlobalState::System.into())
    }

    pub(super) fn with_state(state: ClockState) -> Self {
        Self(ClockInner::State(state))
    }

    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub(crate) fn with_control(clock_control: &ClockControl) -> Self {
        Self::with_state(ClockState::ClockControl(clock_control.clone()))
    }

    /// Creates a new frozen clock.
    ///
    /// This is a convenience method for creating a clock by calling `ClockControl::new().to_clock()`.
    ///
    /// **Note**: The returned clock will not advance time; all time and timers are frozen.
    ///
    /// # Example
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
    /// let timestamp = clock.timestamp();
    /// let instance = clock.instant();
    ///
    /// sleep(Duration::from_micros(1));
    ///
    /// assert_eq!(timestamp, clock.timestamp());
    /// assert_eq!(instance, clock.instant());
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen() -> Self {
        crate::ClockControl::new().to_clock()
    }

    /// Creates a new frozen clock at the specified timestamp.
    ///
    /// This is a convenience method for creating a clock by calling `ClockControl::new_at(time).to_clock()`.
    ///
    /// **Note**: The returned clock will not advance time; all time and timers are frozen at the specified timestamp.
    ///
    /// # Example
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
    /// let timestamp = clock.timestamp();
    /// let system_time = clock.system_time();
    ///
    /// assert_eq!(system_time, specific_time);
    /// assert_eq!(timestamp, clock.timestamp());
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_frozen_at(time: impl Into<crate::ClockTimestamp>) -> Self {
        crate::ClockControl::new_at(time).to_clock()
    }

    /// Retrieves the current absolute time as [`Timestamp`][crate::Timestamp].
    ///
    /// This method provides an enhanced absolute time type with additional capabilities beyond
    /// [`system_time()`][Self::system_time], including:
    /// - Formatting and parsing support through the [`fmt`][crate::fmt] module
    /// - Serialization and deserialization capabilities
    /// - Ability to send time information across process boundaries
    /// - Conversion to and from [`SystemTime`]
    ///
    /// **Note**: The timestamp is not monotonic and can be affected by system clock changes.
    /// When the system clock changes, the current timestamp may be older than a previously
    /// retrieved one.
    ///
    /// For basic absolute time needs without formatting or serialization requirements, consider
    /// using [`system_time()`][Self::system_time] instead. For relative time measurements,
    /// use [`Stopwatch`][super::Stopwatch].
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::Clock;
    ///
    /// # fn retrieve_timestamp(clock: &Clock) {
    /// let timestamp1 = clock.timestamp();
    /// let timestamp2 = clock.timestamp();
    ///
    /// assert!(timestamp2 >= timestamp1);
    /// # }
    /// ```
    #[cfg(any(feature = "timestamp", test))]
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "the panic can never happen because the system time is always within the supported range of the timestamp"
    )]
    pub fn timestamp(&self) -> crate::Timestamp {
        crate::Timestamp::from_system_time(self.system_time())
            .expect("the system time that we convert to a timestamp is never out of supported range of the timestamp")
    }

    /// Retrieves the current absolute time as [`SystemTime`].
    ///
    /// This method provides the standard library's representation of absolute time. Use this when:
    /// - You need basic absolute time functionality
    /// - You need to interoperate with other crates using `SystemTime`
    /// - You don't need formatting, parsing, or serialization capabilities
    ///
    /// For enhanced absolute time capabilities including formatting, parsing, and serialization support,
    /// use [`timestamp()`][Self::timestamp] instead (requires the `timestamp` feature).
    ///
    /// **Note**: The system time is not monotonic and can be affected by system clock changes.
    /// When the system clock changes, the current time may be older than a previously retrieved one.
    /// For relative time measurements, use [`Stopwatch`][super::Stopwatch].
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

    /// Retrieves the current [`Instant`] time.
    ///
    /// The `Instant` represents a monotonic time point guaranteed to always be increasing.
    /// Unlike [`system_time`][Self::system_time], the instant is not affected by system clock
    /// changes and provides a stable reference point for measuring elapsed time.
    ///
    /// **Note**: For time measurements, consider using [`Stopwatch`][super::Stopwatch] instead,
    /// which provides a more convenient API for measuring elapsed time.
    ///
    /// **Important**: When measuring elapsed time with [`Instant`], use [`Instant::duration_since`]
    /// rather than `Instant::elapsed`. The `elapsed` method bypasses the clock and goes directly
    /// to system time, which means it won't respect controlled time in tests or when using
    /// `ClockControl`.
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
        self.0.local_state()
    }
}

impl AsRef<Self> for Clock {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[derive(Debug, Clone)]
enum ClockInner {
    State(ClockState),

    #[cfg(any(feature = "tokio", test))]
    Tokio(TokioClockState),
}

impl ClockInner {
    fn local_state(&self) -> &ClockState {
        match self {
            Self::State(state) => state,
            #[cfg(any(feature = "tokio", test))]
            Self::Tokio(tokio_state) => tokio_state.clock_state(),
        }
    }
}

#[cfg(any(feature = "tokio", test))]
#[derive(Debug, Clone)]
struct TokioClockState {
    state: ClockState,
    cancellation: Arc<()>,
}

#[cfg(any(feature = "tokio", test))]
impl TokioClockState {
    fn new(state: ClockState) -> Self {
        Self {
            state,
            cancellation: Arc::new(()),
        }
    }

    /// Returns a reference to the inner clock state.
    fn clock_state(&self) -> &ClockState {
        &self.state
    }
}

impl From<&Self> for Clock {
    fn from(clock: &Self) -> Self {
        clock.clone()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::arithmetic_side_effects, reason = "no need to be strict in tests")]

    use std::thread::sleep;
    use std::time::Duration;

    use super::*;
    use crate::Delay;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Clock: Send, Sync, AsRef<Clock>);

        // test-util and tokio features are always enabled in tests
        static_assertions::const_assert!(std::mem::size_of::<ClockState>() == 16);
        static_assertions::const_assert!(std::mem::size_of::<ClockInner>() == 24);
        static_assertions::const_assert!(std::mem::size_of::<Clock>() == 24);
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn test_now() {
        let now = std::time::SystemTime::now();

        let clock = Clock::with_frozen_timers();
        let absolute = clock.timestamp();
        assert!(absolute.to_system_time() >= now);
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
        let clock = Clock::with_frozen_timers();
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

        let clock = Clock::with_frozen_timers();
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
        Delay::new(&clock, Duration::from_millis(15)).await;
    }

    #[cfg(not(miri))] // The logic we call talks to the real OS, which Miri cannot do.
    #[tokio::test]
    async fn tokio_ensure_future_finished_when_clock_dropped() {
        let (clock, handle) = Clock::tokio_core();

        Delay::new(&clock, Duration::from_millis(15)).await;

        drop(clock);

        handle.await.unwrap();
    }

    #[test]
    fn new_frozen_ok() {
        let clock = Clock::new_frozen();

        let now = clock.timestamp();
        let instant = clock.instant();

        sleep(Duration::from_micros(1));

        // The frozen clock should return the same timestamp and instant on every call
        assert_eq!(now, clock.timestamp());
        assert_eq!(instant, clock.instant());
    }

    #[test]
    fn new_frozen_at_ok() {
        let specific_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let clock = Clock::new_frozen_at(specific_time);

        let timestamp = clock.timestamp();
        let system_time = clock.system_time();

        sleep(Duration::from_micros(1));

        // The frozen clock should return the same timestamp and system time on every call
        assert_eq!(system_time, specific_time);
        assert_eq!(timestamp, clock.timestamp());
        assert_eq!(system_time, clock.system_time());
    }
}
