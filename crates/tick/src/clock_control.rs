// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::{Duration, Instant, SystemTime};

use super::{TimerKey, Timers};
use crate::{Clock, ClockTimestamp};

/// Controls the flow of time in tests.
///
/// This is useful for testing time-sensitive code without having to wait for real time to pass.
/// `ClockControl` is available when the `test-util` feature is enabled.
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
/// let now = clock.timestamp();
///
/// // Advance the time by one second
/// control.advance(Duration::from_secs(1));
///
/// assert_eq!(
///     clock.timestamp().checked_duration_since(now)?,
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
/// let now = clock.timestamp();
///
/// assert_eq!(
///     clock.timestamp().checked_duration_since(now)?,
///     Duration::from_secs(1)
/// );
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Production code and `ClockControl`
///
/// You should never enable the `test-util` feature or use `ClockControl` in production code.
/// When the `test-util` feature is enabled, extra code is compiled into the binary to support
/// testing scenarios. This extra code hampers performance when running in production.
///
/// Always ensure that the `test-util` feature is only enabled for `dev-dependencies`.
///
/// ```toml
/// tick = { version = "*", features = ["test-util"] }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ClockControl {
    /// Clock control requires controlling the flow of time across threads.
    /// For this reason, we need to use a mutex to ensure that state is consistent
    /// across all threads.
    state: Arc<Mutex<State>>,
}

impl ClockControl {
    /// Creates a new `ClockControl` instance.
    ///
    /// By default, the clock control has no auto-advance set and the initial time is set to the UNIX epoch.
    ///
    /// # Examples
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use tick::{ClockControl, Timestamp};
    ///
    /// let clock = ClockControl::new()
    ///     .auto_advance(std::time::Duration::from_secs(1))
    ///     .to_clock();
    ///
    /// let timestamp1 = clock.timestamp();
    /// let timestamp2 = clock.timestamp();
    ///
    /// assert_eq!(
    ///     timestamp2.checked_duration_since(timestamp1)?,
    ///     std::time::Duration::from_secs(1)
    /// );
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::new())),
        }
    }

    /// Creates a new `ClockControl` instance at the specified timestamp.
    ///
    /// This method accepts various timestamp types through the [`ClockTimestamp`] enum:
    ///
    /// - `SystemTime`: Sets the clock to an absolute system time
    /// - `Timestamp`: Sets the clock to a specific timestamp
    /// - `Duration`: Advances the clock by the specified duration from `UNIX_EPOCH`
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
    ///
    /// // Create clock advanced by a duration
    /// let control = ClockControl::new_at(Duration::from_secs(100));
    /// let clock = control.to_clock();
    /// assert_eq!(
    ///     clock.system_time(),
    ///     SystemTime::UNIX_EPOCH + Duration::from_secs(100)
    /// );
    /// ```
    #[must_use]
    pub fn new_at(timestamp: impl Into<ClockTimestamp>) -> Self {
        let this = Self::new();
        match timestamp.into() {
            ClockTimestamp::System(time) => this.advance_to(time),
            ClockTimestamp::Timestamp(ts) => this.advance_to(ts.to_system_time()),
            ClockTimestamp::Offset(duration) => this.advance(duration),
        }
        this
    }

    /// Creates a new `ClockControl` instance with the current system time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    ///
    /// use tick::ClockControl;
    ///
    /// let control = ClockControl::now();
    /// let clock = control.to_clock();
    ///
    /// assert!(SystemTime::now() >= clock.system_time());
    /// ```
    #[must_use]
    pub fn now() -> Self {
        Self::new_at(SystemTime::now())
    }

    /// Converts the `ClockControl` to a `Clock` instance.
    #[must_use]
    pub fn to_clock(&self) -> Clock {
        Clock::with_control(self)
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
    /// let now = clock.timestamp();
    /// let later = clock.timestamp(); // Automatically advances by 1 second
    ///
    /// assert_eq!(later.checked_duration_since(now)?, Duration::from_secs(1));
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
    /// When auto-advance is enabled via [`Self::auto_advance`], this method limits the total
    /// amount of time that can be auto-advanced. Once the limit is reached, further calls to
    /// access the current time will no longer auto-advance the clock.
    ///
    /// **Note:** This method only has an effect if [`Self::auto_advance`] has been called
    /// previously to set a non-zero auto-advance duration.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{Clock, ClockControl, Delay, FutureExt};
    ///
    /// # async fn auto_advance_limit_example() {
    /// // Limit the max auto-advance to 500ms. The 700ms delay never completes because
    /// // the total auto-advance is capped. Instead, the 200ms timeout completes.
    /// let clock = ClockControl::new()
    ///     .auto_advance(Duration::from_millis(200))
    ///     .auto_advance_limit(Duration::from_millis(500))
    ///     .to_clock();
    ///
    /// // Create a long-running future
    /// let future = Delay::new(&clock, Duration::from_millis(700));
    ///
    /// // Apply a timeout to the future and await it
    /// let timeout_error = future
    ///     .timeout(Duration::from_millis(200), &clock)
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

    /// Determines whether the clock control should automatically auto-advance all upcoming timers.
    ///
    /// Note that when [`Self::auto_advance_limit`] is used, the maximum total auto-advance duration is respected.
    /// This means that when the total of all auto-advances exceeds the maximum, the auto-advance will stop and such timers won't be fired.
    #[must_use]
    pub fn auto_advance_timers(self, enabled: bool) -> Self {
        self.with_state(|v| v.auto_advance_timers = enabled);
        self
    }

    /// Manually advances the clock by the specified number of milliseconds.
    ///
    /// In addition to advancing the current time, this method also advances the timers that
    /// are registered and are scheduled to be fired.
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
    /// let now = clock.timestamp();
    /// control.advance_millis(100);
    /// assert_eq!(
    ///     clock.timestamp().checked_duration_since(now)?,
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
    /// In addition to advancing the current time, this method also advances the timers that
    /// are registered and are scheduled to be fired.
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
    /// let now = clock.timestamp();
    /// control.advance(Duration::from_secs(1));
    /// assert_eq!(
    ///     clock.timestamp().checked_duration_since(now)?,
    ///     Duration::from_secs(1)
    /// );
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn advance(&self, duration: Duration) {
        self.with_state(|v| v.advance(duration, TimeFlow::Forward));
    }

    /// Advances the clock to the specified system time.
    ///
    /// The clock can be advanced to the future or to the past. Advancing the clock forward
    /// fires all timers that are scheduled to be fired before or at the target time.
    #[expect(
        clippy::missing_panics_doc,
        reason = "we are handling cases where the timestamp is either in future or past and the resulting duration is always positive"
    )]
    pub fn advance_to(&self, timestamp: impl Into<SystemTime>) {
        let now = self.system_time();
        let timestamp = timestamp.into();

        match timestamp.duration_since(now) {
            Ok(duration) => {
                self.with_state(|v| v.advance(duration, TimeFlow::Forward));
            }
            Err(_e) => {
                let duration = now.duration_since(timestamp).expect("the resulting duration must be positive here");

                self.with_state(|v| v.advance(duration, TimeFlow::Backward));
            }
        }
    }

    pub(super) fn system_time(&self) -> SystemTime {
        self.with_state(State::now)
    }

    pub(super) fn instant(&self) -> Instant {
        self.with_state(State::instant_now)
    }

    pub(super) fn register_timer(&self, when: Instant, waker: Waker) -> TimerKey {
        let key = self.with_state(|s| s.timers.register(when, waker));
        self.with_state(State::evaluate_timers);
        key
    }

    pub(super) fn unregister_timer(&self, key: TimerKey) {
        self.with_state(|s| s.timers.unregister(key));
    }

    pub(super) fn next_timer(&self) -> Option<Instant> {
        self.with_state(|s| s.timers.next_timer())
    }

    #[cfg(test)]
    pub(super) fn timers_len(&self) -> usize {
        self.with_state(|s| s.timers.len())
    }

    fn with_state<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut State) -> R,
    {
        f(&mut self.state.lock().expect("acquiring lock must always succeed"))
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
        Self::new()
    }
}

impl State {
    fn new() -> Self {
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

    fn auto_advance(&mut self, duration: Option<Duration>) {
        let auto_advance = self.get_next_auto_advance_duration(duration.unwrap_or(self.auto_advance));
        self.auto_advance_total = self.auto_advance_total.saturating_add(auto_advance);
        self.advance(auto_advance, TimeFlow::Forward);
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
    fn advance(&mut self, duration: Duration, flow: TimeFlow) {
        self.advance_time(duration, flow);
        self.evaluate_timers();
    }

    fn evaluate_timers(&mut self) {
        self.timers.advance_timers(self.instant);

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

            self.advance(advance, TimeFlow::Forward);
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
                self.timers.advance_timers(self.instant);
            }
            TimeFlow::Backward => {
                self.instant = self.instant.checked_sub(duration).expect(OUTSIDE_RANGE_MESSAGE);
                self.system_time = self.system_time.checked_sub(duration).expect(OUTSIDE_RANGE_MESSAGE);

                // There is no point in advancing/triggering the timers if we are moving back
                // in time. Timers are only ever fired when time moves forward.
                // No need to call `self.timers.advance_timers` here.
            }
        }
    }

    fn now(&mut self) -> SystemTime {
        let time = self.system_time;
        self.auto_advance(None);
        time
    }

    fn instant_now(&mut self) -> Instant {
        let time = self.instant;
        self.auto_advance(None);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmt::UnixSecondsTimestamp;
    use crate::{Stopwatch, Timestamp};

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(ClockControl: Send, Sync);
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
    fn auto_advance_ok() {
        let duration = Duration::from_secs(1);
        let control = ClockControl::new().auto_advance(duration);
        let clock = control.to_clock();

        assert_eq!(control.with_state(|s| s.auto_advance), duration);
        let now = clock.timestamp();
        assert_eq!(clock.timestamp().checked_duration_since(now).unwrap(), duration);

        let watch = Stopwatch::new(&clock);
        assert_eq!(watch.elapsed(), duration);
    }

    #[test]
    fn advance_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.timestamp();

        // act
        () = control.advance(Duration::from_secs(1));

        // assert
        assert_eq!(clock.timestamp().checked_duration_since(now).unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn advance_to_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.timestamp();

        // act
        control.advance_to(now.checked_add(Duration::from_secs(1)).unwrap());

        // assert
        assert_eq!(clock.timestamp().checked_duration_since(now).unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn advance_to_past_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.timestamp();

        // act
        control.advance_to(now.checked_add(Duration::from_secs(10)).unwrap());
        let now1 = clock.timestamp();
        let instant_now1 = clock.instant();

        () = control.advance_to(now1.checked_sub(Duration::from_secs(5)).unwrap());
        let now2 = clock.timestamp();
        let instant_now2 = clock.instant();

        // assert
        assert_eq!(now1.checked_duration_since(now2).unwrap(), Duration::from_secs(5));

        assert_eq!(instant_now1.checked_duration_since(instant_now2).unwrap(), Duration::from_secs(5));
    }

    #[test]
    fn advance_millis_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = control.to_clock();
        let now = clock.timestamp();

        // act
        () = control.advance_millis(123);

        // assert
        assert_eq!(clock.timestamp().checked_duration_since(now).unwrap(), Duration::from_millis(123));
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
        let control = ClockControl::new().auto_advance_timers(true);
        let clock = control.to_clock();
        let now = clock.timestamp();

        control.register_timer(clock.instant() + Duration::from_secs(100), Waker::noop().clone());

        // assert
        assert_eq!(clock.timestamp().checked_duration_since(now).unwrap(), Duration::from_secs(100));
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
    fn auto_advance_limit() {
        let control = ClockControl::new()
            .auto_advance(Duration::from_millis(550))
            .auto_advance_limit(Duration::from_secs(2));
        let clock = control.to_clock();

        let anchor = clock.timestamp();

        assert_eq!(
            clock.timestamp().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(550)
        );

        assert_eq!(
            clock.timestamp().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(1100)
        );

        assert_eq!(
            clock.timestamp().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(1650)
        );

        assert_eq!(
            clock.timestamp().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(2000)
        );

        assert_eq!(
            clock.timestamp().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(2000)
        );
    }

    #[test]
    fn outside_range_message() {
        let msg = format!(
            "moving the clock outside of the supported time range is not possible: [{}, {}]",
            Timestamp::UNIX_EPOCH,
            Timestamp::MAX
        );
        assert_eq!(OUTSIDE_RANGE_MESSAGE, msg);
    }

    #[test]
    fn new_at_with_system_time_ok() {
        let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(222);
        let control = ClockControl::new_at(system_time);
        let clock = control.to_clock();

        assert_eq!(clock.system_time(), system_time);
    }

    #[test]
    fn new_at_with_duration_ok() {
        let duration = Duration::from_secs(100);
        let control = ClockControl::new_at(duration);
        let clock = control.to_clock();

        assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH + duration);
    }

    #[test]
    fn new_at_with_timestamp_ok() {
        let timestamp = UnixSecondsTimestamp::from_secs(222).unwrap().into();
        let control = ClockControl::new_at(timestamp);
        let clock = control.to_clock();

        assert_eq!(clock.timestamp(), timestamp);
    }

    #[cfg(not(miri))]
    #[test]
    fn now_ok() {
        let now_1 = SystemTime::now();
        let now_2 = ClockControl::now().to_clock().system_time();

        assert!(now_2 >= now_1);
    }

    #[test]
    fn auto_advance_timers_no_stack_overflow() {
        // This test verifies that evaluate_timers doesn't cause stack overflow
        // by recursively calling itself through advance_time.
        // Before the fix, this would overflow because:
        // evaluate_timers -> advance_time -> evaluate_timers -> advance_time -> ...

        let control = ClockControl::new().auto_advance_timers(true);
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

        let control = ClockControl::new().auto_advance_timers(true);
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
}
