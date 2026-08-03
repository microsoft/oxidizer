// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::state::ClockState;
use crate::timers::{TimerKey, Timers};
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

impl Default for ClockControlBuilder {
    fn default() -> Self {
        ClockControl::builder()
    }
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

    /// Sets the duration by which the clock will auto-advance when accessing the current time.
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
    /// let later = clock.system_time(); // Automatically advances by 1 second
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

    /// Sets a limit on the total auto-advance duration.
    ///
    /// This limit applies to the combined advancement from [`Self::auto_advance`] and
    /// [`Self::auto_advance_timers`]. Once the limit is reached, neither reading the current
    /// time nor firing pending timers will automatically advance the clock further.
    ///
    /// > **Note**: At least one automatic advancement mode must be enabled: a non-zero
    /// > [`Self::auto_advance`] duration or [`Self::auto_advance_timers`] set to `true`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{ClockControl, FutureExt};
    ///
    /// # async fn auto_advance_limit_example() {
    /// // Limit the max auto-advance to 500ms. The 700ms delay never completes because
    /// // the total auto-advance is capped. Instead, the 200ms timeout completes.
    /// let clock = ClockControl::new()
    ///     .auto_advance(Duration::from_millis(200))
    ///     .auto_advance_limit(Duration::from_millis(500))
    ///     .to_clock();
    ///
    /// // Create a long-running future and apply a timeout
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

    /// Configures whether the clock automatically advances to fire pending timers.
    ///
    /// When enabled, the clock fast-forwards on its own to fire timers, so timer-based futures
    /// such as [`Delay`](crate::Delay) complete without any manual clock advancement and without
    /// waiting for real time to elapse. This is the simplest way to keep time-dependent tests both
    /// deterministic and fast: awaiting a delay resolves right away in wall-clock terms.
    ///
    /// The clock advances both when a timer is scheduled and when the current time is read, jumping
    /// forward to a timer's deadline before firing it. Simulated time is still consumed, so
    /// elapsed-time measurements (for example via a [`Stopwatch`](crate::Stopwatch)) reflect each
    /// delay's configured duration. A delay with no finite deadline
    /// ([`Duration::MAX`](std::time::Duration::MAX)) never completes.
    ///
    /// # Timers do not run concurrently
    ///
    /// Timers are fired eagerly, one at a time, as they are scheduled; this option does not
    /// simulate multiple timers running side by side. Do not rely on concurrent delays completing
    /// in duration order — a shorter delay scheduled after a longer one may still complete after
    /// it. When the relative ordering of concurrent timers matters, drive the clock explicitly with
    /// [`Self::advance`] instead.
    ///
    /// > **Note**: When [`Self::auto_advance_limit`] is set, the maximum total auto-advance
    /// > duration is respected. Once the limit is reached, no further timers will be fired
    /// > automatically.
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
    /// // Resolves right away in real time — no manual `advance` call is needed.
    /// clock.delay(Duration::from_secs(30)).await;
    ///
    /// // Simulated time still elapses by the delay's duration.
    /// assert_eq!(stopwatch.elapsed(), Duration::from_secs(30));
    /// # }
    /// ```
    #[must_use]
    pub fn auto_advance_timers(self, enabled: bool) -> Self {
        self.with_state(|v| v.auto_advance_timers = enabled);
        self
    }

    /// Manually advances the clock by the specified number of milliseconds.
    ///
    /// In addition to advancing the current time, this method fires any registered timers
    /// that are scheduled to expire within the advanced period.
    ///
    /// When [`Self::auto_advance_timers`] is enabled, the clock can then advance further
    /// to fire pending timers, subject to [`Self::auto_advance_limit`].
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

    /// Manually advances the clock by the specified duration.
    ///
    /// In addition to advancing the current time, this method fires any registered timers
    /// that are scheduled to expire within the advanced period.
    ///
    /// When [`Self::auto_advance_timers`] is enabled, the clock can then advance further
    /// to fire pending timers, subject to [`Self::auto_advance_limit`].
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
        F: FnOnce(&mut State, &mut Vec<Waker>) -> R,
    {
        let mut ready = Vec::new();
        let result = self.with_state(|state| f(state, &mut ready));

        for waker in ready {
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
        let mut ready = Vec::new();
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
    fn set_time(&mut self, timestamp: SystemTime, ready: &mut Vec<Waker>) {
        match timestamp.duration_since(self.system_time) {
            Ok(duration) => self.advance(duration, TimeFlow::Forward, ready),
            Err(error) => self.advance(error.duration(), TimeFlow::Backward, ready),
        }
    }

    fn auto_advance(&mut self, duration: Option<Duration>, ready: &mut Vec<Waker>) {
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
    fn advance(&mut self, duration: Duration, flow: TimeFlow, ready: &mut Vec<Waker>) {
        self.advance_time(duration, flow);
        self.evaluate_timers(ready);
    }

    fn evaluate_timers(&mut self, ready: &mut Vec<Waker>) {
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

    fn now(&mut self, ready: &mut Vec<Waker>) -> SystemTime {
        let time = self.system_time;
        self.auto_advance(None, ready);
        time
    }

    fn instant_now(&mut self, ready: &mut Vec<Waker>) -> Instant {
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
}
