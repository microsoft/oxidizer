// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::state::ClockState;
use crate::timers::{ReadyTimers, TimerKey, Timers};
use crate::{Clock, thread_aware_move};

/// Controls the passage of time in tests.
///
/// This is useful for testing time-sensitive code without having to wait for real time to pass.
/// [`ClockControl`] is available when the `test-util` feature is enabled.
///
/// To create a [`Clock`] from `ClockControl`, use the [`ClockControl::to_clock`] method.
///
/// # Examples
///
/// ## Advancing time manually
/// ```
/// # use std::time::Duration;
/// # use tick::{Clock, ClockControl};
/// let control = ClockControl::new();
/// let clock = control.to_clock();
///
/// let now = clock.system_time();
///
/// // Advance the time by one second
/// control.advance(Duration::from_secs(1));
///
/// assert_eq!(
///     clock.system_time().duration_since(now)?,
///     Duration::from_secs(1)
/// );
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ## Advancing time automatically
/// ```
/// # use std::time::Duration;
/// # use tick::{Clock, ClockControl};
/// let clock = ClockControl::builder()
///     .auto_advance(Duration::from_secs(1))
///     .build()
///     .to_clock();
///
/// let now = clock.system_time();
///
/// assert_eq!(
///     clock.system_time().duration_since(now)?,
///     Duration::from_secs(1)
/// );
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Production code and `ClockControl`
///
/// You should **never** enable the `test-util` feature or use [`ClockControl`] in production code.
/// When the `test-util` feature is enabled, extra code is compiled into the binary to support
/// testing scenarios. This extra code hampers performance when running in production.
///
/// Always ensure that the `test-util` feature is only enabled for `dev-dependencies`.
///
/// ```toml
/// tick = { version = "*", features = ["test-util"] }
/// ```
#[derive(Clone, Default)]
pub struct ClockControl {
    /// Clock control requires controlling the passage of time across threads.
    /// For this reason, we need to use a mutex to ensure that state is consistent
    /// across all threads.
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for ClockControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("ClockControl");

        let time = self.system_time();

        match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => debug.field("UNIX offset", &duration),
            Err(_) => debug.field("UNIX offset", &"negative"),
        };

        debug.field("timers", &self.timers_len()).finish_non_exhaustive()
    }
}

thread_aware_move!(ClockControl);

/// Configures and creates a [`ClockControl`].
///
/// Use [`ClockControl::builder`] when controlled time needs automatic advancement,
/// an advancement limit, or a non-default initial time.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::ClockControl;
///
/// let control = ClockControl::builder()
///     .time(SystemTime::UNIX_EPOCH + Duration::from_secs(10))
///     .auto_advance(Duration::from_secs(1))
///     .build();
///
/// let clock = control.to_clock();
/// assert_eq!(
///     clock.system_time(),
///     SystemTime::UNIX_EPOCH + Duration::from_secs(10)
/// );
/// ```
#[derive(Debug, Clone)]
pub struct ClockControlBuilder {
    time: SystemTime,
    auto_advance: Duration,
    auto_advance_total_max: Option<Duration>,
    auto_advance_timers: bool,
}

impl ClockControl {
    /// Creates a new `ClockControl` instance.
    ///
    /// By default, auto-advance is disabled and the initial time is set to the UNIX epoch.
    ///
    /// # Examples
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::ClockControl;
    ///
    /// let clock = ClockControl::builder()
    ///     .auto_advance(Duration::from_secs(1))
    ///     .build()
    ///     .to_clock();
    ///
    /// let time1 = clock.system_time();
    /// let time2 = clock.system_time();
    ///
    /// assert_eq!(time2.duration_since(time1)?, Duration::from_secs(1));
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a builder for a configured `ClockControl`.
    #[must_use]
    pub fn builder() -> ClockControlBuilder {
        ClockControlBuilder {
            time: SystemTime::UNIX_EPOCH,
            auto_advance: Duration::ZERO,
            auto_advance_total_max: None,
            auto_advance_timers: false,
        }
    }

    /// Creates a clock control that automatically advances pending timers.
    ///
    /// This is a shortcut for
    /// `ClockControl::builder().auto_advance_timers().build()`.
    #[must_use]
    pub fn new_auto_advancing() -> Self {
        Self::builder().auto_advance_timers().build()
    }

    /// Creates a new `ClockControl` instance at the specified time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::ClockControl;
    ///
    /// // Create clock at a specific system time
    /// let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    /// let control = ClockControl::new_at(system_time);
    /// let clock = control.to_clock();
    /// assert_eq!(clock.system_time(), system_time);
    /// ```
    #[must_use]
    pub fn new_at(time: impl Into<SystemTime>) -> Self {
        Self::builder().time(time).build()
    }

    /// Converts this `ClockControl` into a `Clock` instance.
    ///
    /// The returned `Clock` is internally linked to this `ClockControl`. Cloning the `Clock`
    /// preserves this link.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::ClockControl;
    ///
    /// let control = ClockControl::new();
    /// let clock = control.to_clock();
    /// let clock_clone = clock.clone();
    ///
    /// // Advance the clock by 1 second
    /// control.advance(Duration::from_secs(1));
    ///
    /// // Ensure the clock and cloned clock are in sync
    /// assert_eq!(clock.system_time(), clock_clone.system_time());
    /// ```
    #[must_use]
    pub fn to_clock(&self) -> Clock {
        Clock::new(ClockState::ClockControl(self.clone()))
    }

    /// Converts this `ClockControl` into a [`SimpleClock`][crate::SimpleClock] instance.
    ///
    /// The returned [`SimpleClock`][crate::SimpleClock] provides time retrieval only (no timers) and
    /// is driven by this `ClockControl`, just like a [`Clock`] created via
    /// [`to_clock`][Self::to_clock]. Both kinds observe the same controlled time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::ClockControl;
    ///
    /// let control = ClockControl::new();
    /// let simple_clock = control.to_simple_clock();
    ///
    /// let start = simple_clock.system_time();
    /// control.advance(Duration::from_secs(1));
    ///
    /// assert_eq!(
    ///     simple_clock.system_time(),
    ///     start.checked_add(Duration::from_secs(1)).unwrap()
    /// );
    /// ```
    #[must_use]
    pub fn to_simple_clock(&self) -> crate::SimpleClock {
        crate::SimpleClock::from_control(self.clone())
    }

    /// Manually advances the clock by the specified duration.
    ///
    /// In addition to advancing the current time, this method fires any registered timers
    /// that are scheduled to expire within the advanced period.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::ClockControl;
    ///
    /// let control = ClockControl::new();
    /// let clock = control.to_clock();
    ///
    /// let now = clock.system_time();
    /// control.advance(Duration::from_secs(1));
    /// assert_eq!(
    ///     clock.system_time().duration_since(now)?,
    ///     Duration::from_secs(1)
    /// );
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the duration would move the controlled [`SystemTime`] or [`Instant`] outside
    /// the range supported by the platform.
    pub fn advance(&self, duration: Duration) {
        self.with_state_and_wake(|state, ready| state.advance(duration, TimeFlow::Forward, ready));
    }

    /// Sets the clock to the specified system time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::ClockControl;
    ///
    /// let control = ClockControl::new();
    /// let clock = control.to_clock();
    ///
    /// let target = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    /// control.set_time(target);
    ///
    /// assert_eq!(clock.system_time(), target);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `timestamp` would move the controlled [`SystemTime`] or [`Instant`] outside
    /// the range supported by the platform.
    pub fn set_time(&self, timestamp: impl Into<SystemTime>) {
        self.with_state_and_wake(|state, ready| state.set_time(timestamp.into(), ready));
    }

    pub(super) fn system_time(&self) -> SystemTime {
        self.with_state_and_wake(State::now)
    }

    pub(super) fn instant(&self) -> Instant {
        self.with_state_and_wake(State::instant_now)
    }

    pub(super) fn register_timer(&self, when: Instant, waker: Waker) -> TimerKey {
        self.with_state_and_wake(|state, ready| {
            let key = state.timers.register(when, waker);
            state.evaluate_timers(ready);
            key
        })
    }

    pub(super) fn update_timer_waker(&self, key: TimerKey, waker: &Waker) {
        self.with_state(|state| state.timers.update_waker(key, waker));
    }

    pub(super) fn unregister_timer(&self, key: TimerKey) {
        self.with_state(|s| s.timers.unregister(key));
    }

    pub(super) fn next_timer(&self) -> Option<Instant> {
        self.with_state(|s| s.timers.next_timer())
    }

    pub(super) fn timers_len(&self) -> usize {
        self.with_state(|s| s.timers.len())
    }

    fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut State) -> R,
    {
        f(&mut self.state.lock().expect("clock control lock poisoned"))
    }

    fn with_state_and_wake<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut State, &mut ReadyTimers) -> R,
    {
        let mut ready = ReadyTimers::new();
        let result = self.with_state(|state| f(state, &mut ready));

        // A wake may synchronously re-enter clock control, so invoke it after releasing the lock.
        for waker in ready.into_values() {
            waker.wake();
        }

        result
    }

    pub(crate) fn is_unique(&self) -> bool {
        Arc::strong_count(&self.state) == 1
    }
}

impl ClockControlBuilder {
    /// Sets the initial system time.
    ///
    /// The default is [`SystemTime::UNIX_EPOCH`].
    #[must_use]
    pub fn time(mut self, time: impl Into<SystemTime>) -> Self {
        self.time = time.into();
        self
    }

    /// Sets the duration advanced whenever the current time is read.
    #[must_use]
    pub fn auto_advance(mut self, duration: Duration) -> Self {
        self.auto_advance = duration;
        self
    }

    /// Limits the total duration consumed by automatic advancement.
    ///
    /// The limit applies to both read-based auto-advance and timer auto-advance.
    #[must_use]
    pub fn auto_advance_limit(mut self, limit: Duration) -> Self {
        self.auto_advance_total_max = Some(limit);
        self
    }

    /// Enables automatic advancement to pending timer deadlines.
    ///
    /// Timers are fired eagerly, one at a time, as they are scheduled. This does not simulate
    /// concurrent timers; use [`ClockControl::advance`] when timer ordering matters.
    #[must_use]
    pub fn auto_advance_timers(mut self) -> Self {
        self.auto_advance_timers = true;
        self
    }

    /// Builds the configured [`ClockControl`].
    ///
    /// # Panics
    ///
    /// Panics if the initial time is outside the range supported by the platform's
    /// [`SystemTime`] or [`Instant`].
    #[must_use]
    pub fn build(self) -> ClockControl {
        let mut state = State::default();
        let mut ready = ReadyTimers::new();
        state.set_time(self.time, &mut ready);
        debug_assert!(ready.is_empty(), "a newly created clock has no timers to wake");
        state.auto_advance = self.auto_advance;
        state.auto_advance_total_max = self.auto_advance_total_max;
        state.auto_advance_timers = self.auto_advance_timers;

        ClockControl {
            state: Arc::new(Mutex::new(state)),
        }
    }
}

impl From<ClockControl> for Clock {
    fn from(control: ClockControl) -> Self {
        control.to_clock()
    }
}

impl From<&ClockControl> for Clock {
    fn from(control: &ClockControl) -> Self {
        control.to_clock()
    }
}

impl From<ClockControl> for crate::SimpleClock {
    fn from(control: ClockControl) -> Self {
        Self::from_control(control)
    }
}

impl From<&ClockControl> for crate::SimpleClock {
    fn from(control: &ClockControl) -> Self {
        control.to_simple_clock()
    }
}

#[derive(Debug)]
struct State {
    instant: Instant,
    system_time: SystemTime,
    timers: Timers,
    auto_advance: Duration,
    auto_advance_total: Duration,
    auto_advance_timers: bool,
    auto_advance_total_max: Option<Duration>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            instant: Instant::now(),
            system_time: SystemTime::UNIX_EPOCH,
            timers: Timers::default(),
            auto_advance: Duration::ZERO,
            auto_advance_timers: false,
            auto_advance_total: Duration::ZERO,
            auto_advance_total_max: None,
        }
    }
}

impl State {
    fn set_time(&mut self, timestamp: SystemTime, ready: &mut ReadyTimers) {
        match timestamp.duration_since(self.system_time) {
            Ok(duration) => self.advance(duration, TimeFlow::Forward, ready),
            Err(error) => self.advance(error.duration(), TimeFlow::Backward, ready),
        }
    }

    fn auto_advance(&mut self, duration: Option<Duration>, ready: &mut ReadyTimers) {
        let auto_advance = self.get_next_auto_advance_duration(duration.unwrap_or(self.auto_advance));
        self.auto_advance_total = self.auto_advance_total.saturating_add(auto_advance);
        self.advance(auto_advance, TimeFlow::Forward, ready);
    }

    fn get_next_auto_advance_duration(&self, hint: Duration) -> Duration {
        if let Some(max) = self.auto_advance_total_max {
            let remaining = max.saturating_sub(self.auto_advance_total);
            hint.min(remaining)
        } else {
            hint
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    fn advance(&mut self, duration: Duration, flow: TimeFlow, ready: &mut ReadyTimers) {
        self.advance_time(duration, flow);
        self.evaluate_timers(ready);
    }

    fn evaluate_timers(&mut self, ready: &mut ReadyTimers) {
        self.timers.advance_timers(self.instant, ready);

        if !self.auto_advance_timers {
            return;
        }

        // Auto-advance to the next timer while respecting auto_advance duration and max limits
        while let Some(next_timer) = self.timers.next_timer() {
            // Calculate how much time we need to advance to reach the next timer
            let time_to_next_timer = next_timer.saturating_duration_since(self.instant);

            // We need to respect max auto_advance duration
            let advance = self.get_next_auto_advance_duration(time_to_next_timer);

            // No need to advance, break from the loop
            if advance == Duration::ZERO {
                break;
            }

            let previous_instant = self.instant;
            self.advance_time(advance, TimeFlow::Forward);
            assert!(
                self.instant > previous_instant,
                "positive forward advancement must move the clock instant"
            );
            self.auto_advance_total = self.auto_advance_total.saturating_add(advance);
            self.timers.advance_timers(self.instant, ready);
        }
    }

    fn advance_time(&mut self, duration: Duration, flow: TimeFlow) {
        if duration == Duration::ZERO {
            return;
        }

        match flow {
            TimeFlow::Forward => {
                self.instant = self.instant.checked_add(duration).expect(OUTSIDE_RANGE_MESSAGE);
                self.system_time = self.system_time.checked_add(duration).expect(OUTSIDE_RANGE_MESSAGE);
            }
            TimeFlow::Backward => {
                self.instant = self.instant.checked_sub(duration).expect(OUTSIDE_RANGE_MESSAGE);
                self.system_time = self.system_time.checked_sub(duration).expect(OUTSIDE_RANGE_MESSAGE);
            }
        }
    }

    fn now(&mut self, ready: &mut ReadyTimers) -> SystemTime {
        let time = self.system_time;
        self.auto_advance(None, ready);
        time
    }

    fn instant_now(&mut self, ready: &mut ReadyTimers) -> Instant {
        let time = self.instant;
        self.auto_advance(None, ready);
        time
    }
}

#[derive(Debug, Copy, Clone)]
enum TimeFlow {
    Forward,
    Backward,
}

static OUTSIDE_RANGE_MESSAGE: &str =
    "moving the clock outside of the supported time range is not possible: [1970-01-01T00:00:00Z, 9999-12-30T22:00:00.9999999Z]";

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::fmt::UnixSeconds;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(ClockControl: Send, Sync, Clone, Default);
        static_assertions::assert_impl_all!(ClockControlBuilder: Send, Sync, Clone);
    }

    #[test]
    fn defaults_ok() {
        // arrange
        let control = ClockControl::new();

        // act & assert
        assert_eq!(control.with_state(|s| s.auto_advance), Duration::ZERO);
        assert_eq!(control.system_time(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn builder_defaults_ok() {
        let control = ClockControl::builder().build();

        assert_eq!(control.with_state(|s| s.auto_advance), Duration::ZERO);
        assert_eq!(control.system_time(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn auto_advance_ok() {
        let duration = Duration::from_secs(1);
        let control = ClockControl::builder().auto_advance(duration).build();
        let clock = control.to_clock();

        assert_eq!(control.with_state(|s| s.auto_advance), duration);
        let now = clock.system_time();
        assert_eq!(clock.system_time().duration_since(now).unwrap(), duration);

        let watch = clock.stopwatch();
        assert_eq!(watch.elapsed(), duration);
    }

    #[test]
    fn advance_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.system_time();

        // act
        () = control.advance(Duration::from_secs(1));

        // assert
        assert_eq!(clock.system_time().duration_since(now).unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn set_time_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.system_time();

        // act
        control.set_time(now.checked_add(Duration::from_secs(1)).unwrap());

        // assert
        assert_eq!(clock.system_time().duration_since(now).unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn set_time_past_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.system_time();

        // act
        control.set_time(now.checked_add(Duration::from_secs(10)).unwrap());
        let now1 = clock.system_time();
        let instant_now1 = clock.instant();

        () = control.set_time(now1.checked_sub(Duration::from_secs(5)).unwrap());
        let now2 = clock.system_time();
        let instant_now2 = clock.instant();

        // assert
        assert_eq!(now1.duration_since(now2).unwrap(), Duration::from_secs(5));

        assert_eq!(instant_now1.checked_duration_since(instant_now2).unwrap(), Duration::from_secs(5));
    }

    #[test]
    fn register_timer_ok() {
        // arrange
        let control = ClockControl::new();

        // act
        let key = control.register_timer(Instant::now(), Waker::noop().clone());

        // assert
        assert_eq!(control.timers_len(), 1);
        control.unregister_timer(key);
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn next_timer_ok() {
        let control = ClockControl::new();

        assert_eq!(control.next_timer(), None);

        let key = control.register_timer(Instant::now(), Waker::noop().clone());
        assert_eq!(control.next_timer().unwrap(), key.tick());
    }

    #[test]
    fn unregister_timer_ok() {
        // arrange
        let control = ClockControl::new();
        let key = control.register_timer(Instant::now(), Waker::noop().clone());

        // act
        control.unregister_timer(key);

        // assert
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn auto_advance_timers() {
        let control = ClockControl::new_auto_advancing();
        let clock = control.to_clock();
        let now = clock.system_time();

        control.register_timer(clock.instant() + Duration::from_secs(100), Waker::noop().clone());

        // assert
        assert_eq!(clock.system_time().duration_since(now).unwrap(), Duration::from_secs(100));
    }

    #[test]
    fn advance_ensure_timers_advanced() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        control.register_timer(clock.instant() + Duration::from_secs(1), Waker::noop().clone());

        // act
        control.advance(Duration::from_secs(1));

        // assert
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn timer_wakers_run_after_releasing_state_lock() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let lock_was_available = Arc::new(AtomicBool::new(false));
        let waker = Waker::from(Arc::new(ReentrantWaker {
            control: control.clone(),
            lock_was_available: Arc::clone(&lock_was_available),
        }));

        control.register_timer(clock.instant() + Duration::from_secs(1), waker);
        control.advance(Duration::from_secs(1));

        assert!(lock_was_available.load(Ordering::Relaxed));
    }

    #[test]
    fn auto_advance_limit() {
        let control = ClockControl::builder()
            .auto_advance(Duration::from_millis(550))
            .auto_advance_limit(Duration::from_secs(2))
            .build();
        let clock = control.to_clock();

        let anchor = clock.system_time();

        assert_eq!(clock.system_time().duration_since(anchor).unwrap(), Duration::from_millis(550));

        assert_eq!(clock.system_time().duration_since(anchor).unwrap(), Duration::from_millis(1100));

        assert_eq!(clock.system_time().duration_since(anchor).unwrap(), Duration::from_millis(1650));

        assert_eq!(clock.system_time().duration_since(anchor).unwrap(), Duration::from_secs(2));

        assert_eq!(clock.system_time().duration_since(anchor).unwrap(), Duration::from_secs(2));
    }

    #[test]
    fn new_at_with_system_time_ok() {
        let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(222);
        let control = ClockControl::new_at(system_time);
        let clock = control.to_clock();

        assert_eq!(clock.system_time(), system_time);
    }

    #[test]
    fn new_at_with_timestamp_ok() {
        let timestamp: SystemTime = UnixSeconds::from_secs(222).unwrap().into();
        let control = ClockControl::new_at(timestamp);
        let clock = control.to_clock();

        assert_eq!(clock.system_time(), timestamp);
    }

    #[test]
    fn auto_advance_timers_no_stack_overflow() {
        // This test verifies that evaluate_timers doesn't cause stack overflow
        // by recursively calling itself through advance_time.
        // Before the fix, this would overflow because:
        // evaluate_timers -> advance_time -> evaluate_timers -> advance_time -> ...

        let control = ClockControl::new_auto_advancing();
        let clock = control.to_clock();
        let start_instant = clock.instant();

        // Register many timers at the same future time that would cause deep recursion if not handled properly
        let target_time = start_instant + Duration::from_secs(100);
        for _ in 0..100 {
            control.register_timer(target_time, Waker::noop().clone());
        }

        // Time should have advanced to the target time exactly once
        assert_eq!(clock.instant().saturating_duration_since(start_instant), Duration::from_secs(100));

        // All timers should have been triggered and removed
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn auto_advance_timers_many_sequential_no_stack_overflow() {
        // This test verifies that evaluate_timers handles many sequential timer advancements
        // iteratively without stack overflow. The loop-based implementation prevents
        // recursion: evaluate_timers -> advance_time -> timers.advance_timers (not evaluate_timers again)

        let control = ClockControl::new_auto_advancing();
        let clock = control.to_clock();
        let start_instant = clock.instant();

        // Register many timers at different future times in a pattern that requires
        // iterative processing through the while loop
        for i in 1..=1000 {
            control.register_timer(start_instant + Duration::from_millis(i), Waker::noop().clone());
        }

        // Time should have advanced to process all timers
        // The actual time advanced depends on when timers were registered
        // but all timers should have been processed
        assert_eq!(control.timers_len(), 0);

        // Time should have advanced at least to the last timer
        assert!(clock.instant().saturating_duration_since(start_instant) >= Duration::from_millis(1));
    }

    #[test]
    fn from_clock_control_ok() {
        let control = ClockControl::default();
        control.advance(Duration::from_millis(12345));

        let clock_1 = Clock::from(control.clone());
        let clock_2 = Clock::from(&control);

        assert_eq!(clock_1.system_time(), SystemTime::UNIX_EPOCH + Duration::from_millis(12345));
        assert_eq!(clock_1.system_time(), clock_2.system_time());
    }

    #[test]
    fn auto_advance_timers_stops_at_limit() {
        let control = ClockControl::builder()
            .auto_advance_timers()
            .auto_advance(Duration::from_secs(1))
            .auto_advance_limit(Duration::from_secs(1))
            .build();
        let clock = control.to_clock();
        let start_instant = clock.instant();

        control.register_timer(start_instant + Duration::from_secs(2), Waker::noop().clone());

        // Access the clock to trigger auto-advance with timer evaluation
        // The first auto_advance consumes the entire 1 second limit.
        // Then evaluate_timers finds the timer, but get_next_auto_advance_duration
        // returns Duration::ZERO because the limit is exhausted, hitting the break.
        let current_instant = clock.instant();

        assert_eq!(current_instant.saturating_duration_since(start_instant), Duration::from_secs(1));

        // The timer should still be registered since we couldn't advance further to reach it
        assert_eq!(control.timers_len(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn debug_ok() {
        let system = SystemTime::UNIX_EPOCH + Duration::from_secs(123);
        let control = ClockControl::new_at(system);

        let future = control.instant() + Duration::from_secs(100);
        control.register_timer(future, Waker::noop().clone());

        insta::assert_debug_snapshot!(control);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn debug_negative_offset_ok() {
        let system = SystemTime::UNIX_EPOCH - Duration::from_secs(123);
        let control = ClockControl::new_at(system);

        insta::assert_debug_snapshot!(control);
    }

    struct ReentrantWaker {
        control: ClockControl,
        lock_was_available: Arc<AtomicBool>,
    }

    impl std::task::Wake for ReentrantWaker {
        fn wake(self: Arc<Self>) {
            self.lock_was_available
                .store(self.control.state.try_lock().is_ok(), Ordering::Relaxed);
        }
    }
}
