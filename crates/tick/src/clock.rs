// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::{Duration, Instant, SystemTime};

use thread_aware::{Thread, ThreadAware};

use crate::simple_clock::SimpleClock;
use crate::state::ClockState;
use crate::timers::TimerKey;

/// Provides time and timer operations.
///
/// Time-dependent code can be slow or difficult to test. With the `test-util`
/// feature, a clock can use controlled time in tests. Production code has no
/// overhead from this support when the feature is disabled.
///
/// Use a clock to:
///
/// - Get the current absolute time in UTC.
/// - Create a [`Stopwatch`][super::Stopwatch] for elapsed time.
/// - Create a [`Timer`][super::Timer] or [`Delay`][super::Delay].
///
/// # Relative and absolute time
///
/// A clock provides relative and absolute time:
///
/// - [`Stopwatch`][super::Stopwatch] provides monotonic relative time. Use it
///   to measure elapsed time within a process.
/// - [`SystemTime`] provides an absolute UTC time. Use it across process
///   boundaries or with other APIs that accept `SystemTime`. The `fmt` feature
///   adds output formats.
///
/// Absolute time is not monotonic. A system clock change can move it backward.
/// Use [`Stopwatch`][super::Stopwatch] when you do not need an absolute time.
/// Call [`system_time()`][Self::system_time] when you do.
///
/// # Clock construction
///
/// A runtime must drive registered timers. With the `tokio` feature,
/// [`Clock::new_tokio()`] creates a Tokio clock. For another async runtime, use
/// the types in [`runtime`][crate::runtime].
///
/// With the `test-util` feature, tests can use
/// [`ClockControl`][crate::ClockControl] or
/// [`Clock::new_frozen`][crate::Clock::new_frozen]. These clocks use controlled
/// time.
///
/// See the [Testing](#testing) section for more information.
///
/// # Testing
///
/// Real waits make tests slow and nondeterministic. Enable `test-util` and use
/// [`ClockControl`][crate::ClockControl] to advance time without waiting.
///
/// # Cloning and shared state
///
/// Cloning a clock is inexpensive. Clones share registered timers. Controlled
/// clones also share controlled time. A timer or time change through one clone
/// is visible through the others.
///
/// With the `fast-instant` feature, each clone has its own instant-read
/// setting. Precise and fast clones still share timers. Timer scheduling always
/// uses the precise driver source.
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
/// `Clock` implements [`ThreadAware`](thread_aware::ThreadAware). A
/// thread-isolated runtime can use this trait to isolate timer storage.
///
/// Source and destination coordinates must have the same runtime
/// [`Owner`](thread_aware::Owner) to create thread-local storage. A relocation
/// to another owner keeps the current storage. The clock still works, but its
/// storage is not local to the destination.
///
/// Relocation depends on the clock type:
///
/// - A system clock gets one timer set per relocated thread. Clones on the same
///   thread share timers. Clocks on different threads have isolated timers.
///   Each thread needs its own [`ClockDriver`][crate::runtime::ClockDriver].
///
/// - A [`ClockControl`][crate::ClockControl] clock does not relocate its state.
///   All clones share one controlled time state across threads.
///
/// - A Tokio clock from [`Clock::new_tokio()`] does not relocate its state. One
///   background task advances a shared timer set for all clones. Separate
///   that task cannot advance separate thread-local storage.
///
/// For a thread-isolated runtime, clone an
/// [`InactiveClock`][crate::runtime::InactiveClock]. Relocate each clone to its
/// target thread before activation. See [`runtime`][crate::runtime].
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
/// use tick::{Clock, Timer};
///
/// # async fn timer_example(clock: &Clock) {
/// let mut timer = Timer::new(&clock, Duration::from_millis(10));
///
/// loop {
///     timer.next().await;
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

    /// Creates a system clock with frozen timers for internal tests.
    #[cfg(test)]
    pub(super) fn new_system_frozen() -> Self {
        Self::new(ClockState::new_system())
    }

    /// Creates a frozen clock.
    ///
    /// This is equivalent to `ClockControl::new().to_clock()`.
    ///
    /// The returned clock does not advance time or timers.
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

    /// Creates a frozen clock at `time`.
    ///
    /// This is equivalent to `ClockControl::new_at(time).to_clock()`.
    ///
    /// The returned clock stays at the specified time. Its timers are frozen.
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

    /// Selects lower-overhead [`Instant`] reads where supported.
    ///
    /// The setting applies only to this clock value. A clone can use a different
    /// setting. A stopwatch keeps the setting of its source clock.
    ///
    /// On Linux and Windows, this option uses a lower-precision source. The
    /// platform defines its resolution. On Windows, the resolution is normally
    /// 10 to 16 milliseconds. Other platforms use [`Instant::now`].
    ///
    /// Timer scheduling still uses the precise driver source. This option does
    /// not affect controlled clocks.
    ///
    /// # Performance
    ///
    /// Enable fast instant reads only if measurements show that
    /// [`Clock::instant`] is a performance bottleneck. Otherwise, use the
    /// default precise source.
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

    /// Returns the current system time.
    ///
    /// [`SystemTime`] is not monotonic. A system clock change can make a later
    /// read older than an earlier read. Use [`Stopwatch`][super::Stopwatch] to
    /// measure elapsed time.
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

    /// Returns the current system time as `T`.
    ///
    /// This method calls [`system_time()`][Self::system_time] and converts the
    /// result. It is intentionally infallible for practical production use.
    /// Production values come from the operating system clock, and all
    /// `tick::fmt` types cover that normalized current-time range. Requiring
    /// every caller to handle a conversion error would add an error path that
    /// supported production formats cannot reach.
    ///
    /// Use [`SimpleClock::system_time_as`][crate::SimpleClock::system_time_as]
    /// for the same conversion without timer support.
    ///
    /// # Type Parameters
    ///
    /// - `T` is a target type that implements [`TryFrom<SystemTime>`]. This can
    ///   be a timestamp type from another crate.
    ///
    /// # Panics
    ///
    /// Panics if the target type cannot represent the current system time.
    /// Supported `tick` formats must represent normalized production clock
    /// data. A panic means that the target range is unsupported or a controlled
    /// test clock is outside that range.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use tick::Clock;
    ///
    /// # fn example(clock: &Clock) {
    /// let time = clock.system_time_as::<SystemTime>();
    /// assert_eq!(time, clock.system_time());
    /// # }
    /// ```
    #[must_use]
    pub fn system_time_as<T: TryFrom<SystemTime>>(&self) -> T {
        self.simple_clock().system_time_as()
    }

    /// Returns the current [`Instant`].
    ///
    /// An [`Instant`] does not decrease. Consecutive reads can be equal. System
    /// clock changes do not affect it, so it is suitable for elapsed time.
    ///
    /// [`Stopwatch`][super::Stopwatch] provides a shorter API for elapsed time.
    ///
    /// When you measure elapsed time with [`Instant`], use
    /// [`Instant::duration_since`]. Do not use `Instant::elapsed`. That method
    /// bypasses this clock and does not follow controlled time.
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

    /// Creates a [`Delay`][crate::Delay] for `duration`.
    ///
    /// This method calls [`Delay::new`][crate::Delay::new].
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

    /// Creates a [`Stopwatch`][crate::Stopwatch] and starts it.
    ///
    /// This method calls [`Stopwatch::new`][crate::Stopwatch::new].
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

    /// Returns time in the comparison domain of this clock's timers.
    ///
    /// Controlled timers follow controlled time. System timers use the precise
    /// driver source. A coarse read can lag after the driver wakes a deadline.
    /// That lag can leave a future pending with no later wake.
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
        let removed = match self.clock_state() {
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(control) => control.unregister_timer(key),
            ClockState::System(storage) => storage.with_timers(|timers| timers.unregister(key)),
        };
        drop(removed);
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

    /// Returns the time-only [`SimpleClock`] view.
    ///
    /// [`SimpleClock`] reads time but does not create timers. Use this view for
    /// an API that only reads time, such as [`Stopwatch`][crate::Stopwatch].
    ///
    /// `Clock` also implements [`AsRef<SimpleClock>`]. You can pass it directly
    /// to an API that accepts this view.
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
