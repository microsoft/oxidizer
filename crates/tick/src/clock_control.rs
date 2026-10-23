// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::state::ClockState;
use crate::timers::{ReadyWakers, TimerKey, Timers};
use crate::{Clock, thread_aware_move};

/// Controls the passage of time in tests.
///
/// Use this type to test time-dependent code without a real wait. Enable the
/// `test-util` feature to use [`ClockControl`].
///
/// Call [`ClockControl::to_clock`] to create a [`Clock`].
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
/// let clock = ClockControl::new()
///     .auto_advance(Duration::from_secs(1))
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
/// # Production use
///
/// Do not enable `test-util` or use [`ClockControl`] in production code. The
/// feature adds test support to the binary and reduces production performance.
///
/// Enable `test-util` only in `dev-dependencies`.
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

impl ClockControl {
    /// Creates a `ClockControl`.
    ///
    /// Automatic advancement is disabled. The initial time is the Unix epoch.
    ///
    /// # Examples
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::ClockControl;
    ///
    /// let clock = ClockControl::new()
    ///     .auto_advance(Duration::from_secs(1))
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
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Creates a `ClockControl` at `time`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::ClockControl;
    ///
    /// // Create a clock at a specific system time.
    /// let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    /// let control = ClockControl::new_at(system_time);
    /// let clock = control.to_clock();
    /// assert_eq!(clock.system_time(), system_time);
    /// ```
    #[must_use]
    pub fn new_at(time: impl Into<SystemTime>) -> Self {
        let this = Self::new();
        this.set_time(time.into());
        this
    }

    /// Creates a [`Clock`] that this control drives.
    ///
    /// The clock and its clones stay linked to this control.
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
    /// // Both clocks observe the same controlled time.
    /// assert_eq!(clock.system_time(), clock_clone.system_time());
    /// ```
    #[must_use]
    pub fn to_clock(&self) -> Clock {
        Clock::new(ClockState::ClockControl(self.clone()))
    }

    /// Creates a [`SimpleClock`][crate::SimpleClock] that this control drives.
    ///
    /// The returned clock only reads time and has no timers. A [`Clock`] from
    /// [`to_clock`][Self::to_clock] and this `SimpleClock` observe the same
    /// controlled time.
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

    /// Sets how far each time read advances the clock.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::ClockControl;
    ///
    /// let clock = ClockControl::new()
    ///     .auto_advance(Duration::from_secs(1))
    ///     .to_clock();
    ///
    /// let now = clock.system_time();
    /// let later = clock.system_time(); // Advances by 1 second.
    ///
    /// assert_eq!(later.duration_since(now)?, Duration::from_secs(1));
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn auto_advance(self, duration: Duration) -> Self {
        self.with_state(|v| v.auto_advance = duration);
        self
    }

    /// Limits the total automatic advancement.
    ///
    /// The limit includes advancement from [`Self::auto_advance`] and
    /// [`Self::auto_advance_timers`]. After the clock reaches the limit, time
    /// reads and pending timers do not advance it automatically.
    ///
    /// Enable at least one automatic mode. Set a nonzero
    /// [`Self::auto_advance`] duration or set [`Self::auto_advance_timers`] to
    /// `true`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{ClockControl, FutureExt};
    ///
    /// # async fn auto_advance_limit_example() {
    /// // Limit total automatic advancement to 500 milliseconds.
    /// // The 700-millisecond delay cannot complete before the 200-millisecond timeout.
    /// let clock = ClockControl::new()
    ///     .auto_advance(Duration::from_millis(200))
    ///     .auto_advance_limit(Duration::from_millis(500))
    ///     .to_clock();
    ///
    /// // Apply a timeout to a long-running future.
    /// let timeout_error = clock
    ///     .delay(Duration::from_millis(700))
    ///     .timeout(&clock, Duration::from_millis(200))
    ///     .await
    ///     .unwrap_err();
    ///
    /// assert_eq!(timeout_error.to_string(), "future timed out");
    /// # }
    /// ```
    #[must_use]
    pub fn auto_advance_limit(self, limit: Duration) -> Self {
        self.with_state(|v| {
            v.auto_advance_total_max = Some(limit);
        });

        self
    }

    /// Selects whether the clock advances to fire pending timers.
    ///
    /// If enabled, the clock advances to timer deadlines. A timer future such
    /// as [`Delay`](crate::Delay) can complete without a manual clock change or
    /// a real wait.
    ///
    /// The clock advances when a timer is scheduled and when code reads the
    /// current time. It moves to a timer deadline before it fires that timer.
    /// Simulated time still passes, so [`Stopwatch`](crate::Stopwatch) measures
    /// the configured delay. A [`Duration::MAX`](std::time::Duration::MAX)
    /// delay has no finite deadline and never completes.
    ///
    /// # Timers do not run concurrently
    ///
    /// Timers fire one at a time when they are scheduled. This mode does not
    /// simulate concurrent timers. Do not depend on concurrent delays completing
    /// in duration order. A short delay scheduled after a long delay can
    /// complete later. If order matters, call [`Self::advance`] directly.
    ///
    /// If [`Self::auto_advance_limit`] is set, automatic advancement stops at
    /// that total. No later timer fires automatically.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::ClockControl;
    ///
    /// # async fn auto_advance_timers_example() {
    /// let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    ///
    /// let stopwatch = clock.stopwatch();
    ///
    /// // This completes without a real wait or a manual `advance` call.
    /// clock.delay(Duration::from_secs(30)).await;
    ///
    /// // The delay duration still passes in simulated time.
    /// assert_eq!(stopwatch.elapsed(), Duration::from_secs(30));
    /// # }
    /// ```
    #[must_use]
    pub fn auto_advance_timers(self, enabled: bool) -> Self {
        self.with_state(|v| v.auto_advance_timers = enabled);
        self
    }

    /// Advances the clock by `millis` milliseconds.
    ///
    /// The method also fires registered timers due during this period.
    ///
    /// If [`Self::auto_advance_timers`] is enabled, the clock can then advance
    /// farther to fire pending timers. [`Self::auto_advance_limit`] still
    /// applies.
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
    /// control.advance_millis(100);
    /// assert_eq!(
    ///     clock.system_time().duration_since(now)?,
    ///     Duration::from_millis(100)
    /// );
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn advance_millis(&self, millis: u64) {
        self.advance(Duration::from_millis(millis));
    }

    /// Advances the clock by `duration`.
    ///
    /// The method also fires registered timers due during this period.
    ///
    /// If [`Self::auto_advance_timers`] is enabled, the clock can then advance
    /// farther to fire pending timers. [`Self::auto_advance_limit`] still
    /// applies.
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
    /// Panics if the duration moves the controlled [`SystemTime`] or [`Instant`]
    /// outside the platform range.
    pub fn advance(&self, duration: Duration) {
        self.with_state_and_wake(|state, ready| state.advance(duration, TimeFlow::Forward, ready));
    }

    /// Sets the clock to `timestamp`.
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
    /// Panics if `timestamp` moves the controlled [`SystemTime`] or [`Instant`]
    /// outside the platform range.
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

    pub(super) fn update_timer_waker(&self, key: TimerKey, replacement: Waker) -> (bool, Waker) {
        self.with_state(|state| state.timers.update_waker(key, replacement))
    }

    pub(super) fn unregister_timer(&self, key: TimerKey) {
        self.with_state(|s| s.timers.unregister(key));
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
        F: FnOnce(&mut State, &mut ReadyWakers) -> R,
    {
        let mut ready = ReadyWakers::new();
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
    fn set_time(&mut self, timestamp: SystemTime, ready: &mut ReadyWakers) {
        match timestamp.duration_since(self.system_time) {
            Ok(duration) => self.advance(duration, TimeFlow::Forward, ready),
            Err(error) => self.advance(error.duration(), TimeFlow::Backward, ready),
        }
    }

    fn auto_advance(&mut self, duration: Option<Duration>, ready: &mut ReadyWakers) {
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
    fn advance(&mut self, duration: Duration, flow: TimeFlow, ready: &mut ReadyWakers) {
        self.advance_time(duration, flow);
        self.evaluate_timers(ready);
    }

    fn evaluate_timers(&mut self, ready: &mut ReadyWakers) {
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

    fn now(&mut self, ready: &mut ReadyWakers) -> SystemTime {
        let time = self.system_time;
        self.auto_advance(None, ready);
        time
    }

    fn instant_now(&mut self, ready: &mut ReadyWakers) -> Instant {
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

    #[test]
    fn private_zero_advance_does_not_evaluate_ready_timer_storage() {
        // Public zero-duration delays bypass timer registration, so this raw state branch is private.
        let mut state = State::default();
        state.timers.register(state.instant, Waker::noop().clone());

        state.advance_time(Duration::ZERO, TimeFlow::Forward);

        assert_eq!(state.timers.len(), 1);
    }

    #[test]
    fn private_same_deadline_timer_storage_is_processed_iteratively() {
        // Public auto-advance fires each timer eagerly as it is scheduled, so only direct
        // storage access can arrange many timers at one identical absolute deadline.
        let control = ClockControl::new().auto_advance_timers(true);
        let clock = control.to_clock();
        let start_instant = clock.instant();

        let target_time = start_instant + Duration::from_secs(100);
        for _ in 0..100 {
            control.register_timer(target_time, Waker::noop().clone());
        }

        assert_eq!(clock.instant().saturating_duration_since(start_instant), Duration::from_secs(100));
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
    fn displaced_wakers_are_dropped_after_releasing_state_lock() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let lock_was_available = Arc::new(AtomicBool::new(false));
        let waker = Waker::from(Arc::new(ReentrantDropWaker {
            control,
            lock_was_available: Arc::clone(&lock_was_available),
        }));
        let key = clock.register_timer(clock.instant() + Duration::from_secs(1), waker);

        assert!(clock.update_timer_waker(key, Waker::noop()));
        assert!(lock_was_available.load(Ordering::Relaxed));
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

    struct ReentrantDropWaker {
        control: ClockControl,
        lock_was_available: Arc<AtomicBool>,
    }

    impl std::task::Wake for ReentrantDropWaker {
        fn wake(self: Arc<Self>) {
            self.lock_was_available
                .store(self.control.state.try_lock().is_ok(), Ordering::Relaxed);
        }
    }

    impl Drop for ReentrantDropWaker {
        fn drop(&mut self) {
            self.lock_was_available
                .store(self.control.state.try_lock().is_ok(), Ordering::Relaxed);
        }
    }
}
