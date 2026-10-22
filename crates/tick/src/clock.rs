// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::{Duration, Instant, SystemTime};

use thread_aware::{Thread, ThreadAware};

use crate::simple_clock::SimpleClock;
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
/// for Tokio via [`Clock::new_tokio()`] (available with the `tokio` feature). For other async runtimes,
/// you can use types in the [`runtime`][crate::runtime] module to drive the clock.
///
/// In tests, the clock can be constructed directly using [`ClockControl`][crate::ClockControl] or via
/// [`Clock::new_frozen`][crate::Clock::new_frozen]
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
/// this crate provides a [`ClockControl`] type that can be used to control time.
///
/// # Cloning and shared state
///
/// Cloning a clock is inexpensive and every clone shares the same underlying state,
/// including registered timers and, when the `test-util` feature is enabled, the controlled passage of time.
/// Any timers you register or time adjustments you perform through one clone are visible to every other clone
/// created from the same clock.
///
/// With the `fast-instant` feature, the instant retrieval setting is local to each clone. This allows
/// precise and fast clones to share timers while independently selecting their instant source.
/// Timer scheduling remains on the driver's precise time source.
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
/// # Thread-aware relocation
///
/// `Clock` implements [`ThreadAware`](thread_aware::ThreadAware), enabling per-thread timer isolation
/// in thread-isolated runtime architectures.
///
/// Per-thread locality is established only when the source and destination
/// coordinates have the same runtime [`Owner`](thread_aware::Owner). Relocation
/// across owners keeps the current timer storage: the clock remains functional,
/// but its storage is not made local to the destination.
///
/// How relocation affects the clock depends on the underlying clock variant:
///
/// - **System clocks**: Relocation creates per-thread timer storage. After relocation, each thread
///   maintains its own independent set of timers, eliminating cross-thread lock contention. Clones
///   of a clock on the same thread share timers, while clocks relocated to different threads are
///   fully isolated. Each thread's timers must be advanced by its own
///   [`ClockDriver`][crate::runtime::ClockDriver].
///
/// - **`ClockControl` clocks** (`test-util`): Relocation is a no-op. All clones share the same
///   controlled time state regardless of which thread they are on. This is intentional, a single
///   [`ClockControl`][crate::ClockControl] controls time for all clocks derived from it, even
///   across threads.
///
/// - **Tokio clocks** (created via [`Clock::new_tokio()`]): Relocation is a no-op. The Tokio clock
///   is driven by a single background task that advances a shared set of timers, so all clones
///   share the same timer storage regardless of which thread they are on. Per-thread relocation
///   would create independent timer storage on the destination thread that the background driver
///   does not advance, so relocation is intentionally suppressed.
///
/// For thread-isolated setups, the typical pattern is to clone an
/// [`InactiveClock`][crate::runtime::InactiveClock], relocate each clone to its target thread,
/// and then activate it. See the [`runtime`][crate::runtime] module for details.
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
#[derive(Clone)]
pub struct Clock {
    state: ClockState,
    time: SimpleClock,
}

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.state {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(_) => "controlled",
            ClockState::System(_) => "system",
        };

        f.debug_struct("Clock")
            .field("kind", &kind)
            .field("timers", &self.state.timers_len())
            .field("alive", &self.state.alive())
            .finish_non_exhaustive()
    }
}

impl ThreadAware for Clock {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.state.relocate(source, destination);
    }
}

impl Clock {
    pub(crate) fn new(mut state: ClockState) -> Self {
        let time = SimpleClock::from_state(&state);
        state.activate_clock_owner();
        Self { state, time }
    }

    /// Used for testing. For this clock, timers do not advance.
    #[cfg(test)]
    pub(super) fn new_system_frozen() -> Self {
        Self::new(ClockState::new_system())
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

    /// Configures this clock to use lower-overhead [`Instant`] retrieval where supported.
    ///
    /// The setting belongs to this clock instance. A differently configured clone can be used
    /// independently, and stopwatches retain the setting of the clock they were created from.
    ///
    /// On Linux and Windows, enabling this option uses a lower-precision source whose resolution is
    /// platform-dependent; on Windows it is normally 10 to 16 milliseconds. Timer scheduling
    /// continues to use the clock driver's precise time source and is unaffected by this setting.
    /// On other platforms, fast retrieval delegates to [`Instant::now`]. Controlled clocks are
    /// unaffected.
    ///
    /// # Performance
    ///
    /// Only enable fast instant retrieval when instrumentation shows that the default
    /// [`Clock::instant`] retrieval is a performance bottleneck. Otherwise, retain the default
    /// precise source.
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::Clock;
    ///
    /// # fn configure_clock(clock: Clock) {
    /// let precise_clock = clock;
    /// let fast_clock = precise_clock.clone().with_fast_instant(true);
    ///
    /// let precise_watch = precise_clock.stopwatch();
    /// let fast_watch = fast_clock.stopwatch();
    /// # }
    /// ```
    #[cfg(any(feature = "fast-instant", test))]
    #[must_use]
    pub fn with_fast_instant(mut self, enabled: bool) -> Self {
        self.time = self.time.with_fast_instant(enabled);
        self
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
        self.simple_clock().system_time()
    }

    /// Retrieves the current system time and converts it to `T`.
    ///
    /// This is a convenience method that retrieves the current [`SystemTime`] via
    /// [`system_time()`][Self::system_time] and converts it to the specified target type.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The target type that implements [`TryFrom<SystemTime>`]. Common examples include
    ///   timestamp types from external crates that can be constructed from a [`SystemTime`].
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
    /// use tick::Clock;
    ///
    /// # fn example(clock: &Clock) -> Result<(), std::convert::Infallible> {
    /// let time = clock.try_system_time_as::<SystemTime>()?;
    /// assert_eq!(time, clock.system_time());
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_system_time_as<T: TryFrom<SystemTime>>(&self) -> Result<T, T::Error> {
        self.simple_clock().try_system_time_as()
    }

    /// Retrieves the current [`Instant`] time.
    ///
    /// An [`Instant`] represents a monotonically non-decreasing time point. Consecutive reads may be
    /// equal, but later reads do not go backward. Unlike [`system_time`][Self::system_time], the
    /// instant is not affected by system clock changes and provides a stable reference point for
    /// measuring elapsed time.
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
        self.simple_clock().instant()
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
    #[must_use = "futures do nothing unless awaited or polled"]
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

    /// Returns time in the same comparison domain used to advance this clock's timers.
    ///
    /// Controlled timers must follow their manually advanced clock. System timers must instead
    /// use the precise source used by the driver: a coarse read can still lag after the driver
    /// removes and wakes a deadline, leaving the future pending without another wake-up.
    pub(super) fn timer_instant(&self) -> Instant {
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
            ClockState::System(storage) => storage.register_timer(when, waker),
        }
    }

    pub(super) fn unregister_timer(&self, key: TimerKey) {
        match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.unregister_timer(key),
            ClockState::System(storage) => storage.with_timers(|timers| timers.unregister(key)),
        }
    }

    pub(super) fn update_timer_waker(&self, key: TimerKey, waker: &Waker) -> bool {
        let replacement = waker.clone();
        let (exists, displaced) = match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.update_timer_waker(key, replacement),
            ClockState::System(timers) => timers.with_timers(|timers| timers.update_waker(key, replacement)),
        };
        drop(displaced);
        exists
    }

    pub(crate) fn clock_state(&self) -> &ClockState {
        &self.state
    }

    /// Returns the [`SimpleClock`] view of this clock.
    ///
    /// A [`SimpleClock`] exposes only time retrieval (no timers), and is the common abstraction
    /// shared by all clock kinds. Use it to pass this clock to APIs — such as
    /// [`Stopwatch`][crate::Stopwatch] — that only need to read the current time.
    ///
    /// This clock also implements [`AsRef<SimpleClock>`], so it can be passed directly to such
    /// APIs without calling this method explicitly.
    #[must_use]
    pub fn simple_clock(&self) -> &SimpleClock {
        &self.time
    }
}

impl AsRef<Self> for Clock {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AsRef<SimpleClock> for Clock {
    fn as_ref(&self) -> &SimpleClock {
        &self.time
    }
}

impl From<Clock> for SimpleClock {
    fn from(clock: Clock) -> Self {
        clock.time
    }
}

impl From<&Clock> for SimpleClock {
    fn from(clock: &Clock) -> Self {
        clock.time.clone()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)] // Talks to the real OS clock, which Miri cannot do.
    #[test]
    fn private_simple_clock_state_tracks_fast_instant_configuration() {
        // The selected private `TimeKind` variant is not observable through public behavior.
        let precise_clock = Clock::new_system_frozen();
        let fast_clock = precise_clock.clone().with_fast_instant(true);

        assert!(!precise_clock.simple_clock().uses_fast_instant());
        assert!(fast_clock.simple_clock().uses_fast_instant());
        _ = precise_clock.instant();
        _ = fast_clock.instant();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn private_system_constructor_debug_output_reports_an_inactive_driver() {
        // No supported public API creates a system clock without activating its driver.
        let clock = Clock::new_system_frozen();

        assert_eq!(
            format!("{clock:#?}"),
            "Clock {\n    kind: \"system\",\n    timers: 0,\n    alive: false,\n    ..\n}"
        );
    }
}
