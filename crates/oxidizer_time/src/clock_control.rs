// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::{Duration, Instant};

use super::{TimerKey, Timers, Timestamp};

/// The clock control allows controlling the flow of time in tests.
///
/// This is useful for testing time-sensitive code without having to wait for real time to pass.
/// The `ClockControl` is available when the `fakes` feature is enabled for the `oxidizer` crate.
///
/// To control the flow of time, use [`Clock::with_control`][super::Clock::with_control]
/// constructor to create a clock instance.
///
///
/// ``` rust
/// use oxidizer_time::{Clock, ClockControl};
/// use std::time::Duration;
///
/// let mut control = ClockControl::new();
/// let clock = Clock::with_control(&control);
///
/// let now = clock.now();
///
/// // Advance the time by one second
/// control.advance(Duration::from_secs(1));
///
/// assert_eq!(
///     clock.now().checked_duration_since(now)?,
///     Duration::from_secs(1));
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Production code and `ClockControl`
///
/// You should never enable the `fakes` feature and use `ClockControl` in production code.
/// When the `fakes` feature is enabled, extra code is compiled into the binary to support the
/// testing scenarios. This extra code hampers the performance when running in production.
///
/// Always make sure that the `fakes` feature is only ever enabled for `dev-dependencies`.
///
/// ``` toml
/// oxidizer = { version = "*", features = ["fakes"] }
/// ```
#[derive(Debug, Clone)]
pub struct ClockControl {
    /// Clock control requires to control the flow of time across threads.
    /// For this reason, we need to use the mutex to ensure that state is consistent
    /// across all threads.
    state: Arc<Mutex<State>>,
}

impl Default for ClockControl {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockControl {
    /// Creates a new `ClockControl` instance.
    ///
    /// By default, the clock control has no auto-advance set and the initial time is set to UNIX epoch.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxidizer_time::{Clock, ClockControl, Timestamp};
    ///
    /// let control = ClockControl::new().auto_advance(std::time::Duration::from_secs(1));
    /// let clock = Clock::with_control(&control);
    ///
    /// let timestamp1 = clock.now();
    /// let timestamp2 = clock.now();
    ///
    /// assert_eq!(timestamp2.checked_duration_since(timestamp1)?, std::time::Duration::from_secs(1));
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::new())),
        }
    }

    /// Sets the duration by which the clock will auto-advance when accessing the current time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxidizer_time::{Clock, ClockControl};
    ///
    /// let mut control = ClockControl::new().auto_advance(Duration::from_secs(1));
    ///
    /// let clock = Clock::with_control(&control);
    /// let now = clock.now();
    /// let later = clock.now(); // automatically advances by 1 second
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

    /// Sets the duration by which the clock will auto-advance when accessing the current time
    /// alongside the maximum total auto-advance duration.
    ///
    /// This method is useful when you want to ensure that the total auto-advance duration doesn't exceed a certain limit.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidizer_time::{Clock, FutureExt, Delay, ClockControl};
    /// use std::time::Duration;
    ///
    /// async fn auto_advance_with_max_example() {
    ///     // Here, we limit the max auto-advance to 600ms. This means that the delay_future of 700ms never completes.
    ///     // Instead, the timeout_future of 200ms completes by auto-advancing.
    ///     let control = ClockControl::new().auto_advance_with_max(
    ///         Duration::from_millis(200),
    ///         Duration::from_millis(500));
    ///
    ///     let clock = Clock::with_control(&control);
    ///
    ///     // Create a long-running future
    ///     let future = Delay::with_clock(&clock, Duration::from_millis(700));
    ///
    ///     // Apply a timeout to the future and await it
    ///     let timeout_error = future.timeout_with_clock(Duration::from_millis(200), &clock).await.unwrap_err();
    ///
    ///     assert_eq!(timeout_error.to_string(), "future timed out");
    /// }
    /// # futures::executor::block_on(auto_advance_with_max_example());
    /// ```
    #[must_use]
    pub fn auto_advance_with_max(self, duration: Duration, max: Duration) -> Self {
        self.with_state(|v| {
            v.auto_advance = duration;
            v.auto_advance_total_max = Some(max);
        });

        self
    }

    /// Determines whether the clock control should automatically auto-advance all upcoming timers.
    ///
    /// Note that when [`Self::auto_advance_with_max`] is used the maximum total auto-advance duration is respected.
    /// This means that when the total of all auto-advances exceeds the maximum, the auto-advance will stop and such timers won't be fired.
    #[must_use]
    pub fn auto_advance_timers(self, drain: bool) -> Self {
        self.with_state(|v| v.auto_advance_timers = drain);
        self
    }

    /// Manually advances the clock by the specified milliseconds.
    ///
    /// On top of advancing the current time, this method also advances the timers that
    /// are registered and are scheduled to be fired.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxidizer_time::{Clock, ClockControl};
    ///
    /// let mut control = ClockControl::new();
    /// let clock = Clock::with_control(&control);
    ///
    /// let now = clock.now();
    /// control.advance_millis(100);
    /// assert_eq!(clock.now().checked_duration_since(now)?, Duration::from_millis(100));
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn advance_millis(&self, millis: u64) {
        self.advance(Duration::from_millis(millis));
    }

    /// Manually advances the clock by the specified duration.
    ///
    /// On top of advancing the current time, this method also advances the timers that
    /// are registered and are scheduled to be fired.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxidizer_time::{Clock, ClockControl};
    ///
    /// let mut control = ClockControl::new();
    /// let clock = Clock::with_control(&control);
    ///
    /// let now = clock.now();
    /// control.advance(Duration::from_secs(1));
    /// assert_eq!(clock.now().checked_duration_since(now)?, Duration::from_secs(1));
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn advance(&self, duration: Duration) {
        self.with_state(|v| v.advance(duration, TimeFlow::Forward));
    }

    /// Advances the clock to the specified timestamp.
    ///
    /// The clock can be advanced to the future or to the past. Advancing the clock forward also
    /// fires all timers that are scheduled to be fired.
    ///
    /// # Panics
    ///
    /// Panics when the `timestamp` parameter refers to a timestamp that is less than  current time
    /// of clock control.
    pub fn advance_to(&self, timestamp: Timestamp) {
        let now = self.now();

        match timestamp.checked_duration_since(now) {
            Ok(duration) => {
                self.with_state(|v| v.advance(duration, TimeFlow::Forward));
            }
            Err(_e) => {
                let duration = now
                    .checked_duration_since(timestamp)
                    .expect("the resulting duration must be positive here");

                self.with_state(|v| v.advance(duration, TimeFlow::Backward));
            }
        }
    }

    pub(super) fn now(&self) -> Timestamp {
        self.with_state(State::now)
    }

    pub(super) fn instant_now(&self) -> Instant {
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
        f(&mut self
            .state
            .lock()
            .expect("acquiring lock must always succeed"))
    }
}

#[derive(Debug)]
struct State {
    instant: Instant,
    timestamp: Timestamp,
    timers: Timers,
    auto_advance: Duration,
    auto_advance_total: Duration,
    auto_advance_timers: bool,
    auto_advance_total_max: Option<Duration>,
}

impl State {
    fn new() -> Self {
        Self {
            instant: Instant::now(),
            timestamp: Timestamp::UNIX_EPOCH,
            timers: Timers::default(),
            auto_advance: Duration::ZERO,
            auto_advance_timers: false,
            auto_advance_total: Duration::ZERO,
            auto_advance_total_max: None,
        }
    }

    fn auto_advance(&mut self, duration: Option<Duration>) {
        let auto_advance =
            self.get_next_auto_advance_duration(duration.unwrap_or(self.auto_advance));
        self.auto_advance_total = self.auto_advance_total.saturating_add(auto_advance);
        self.advance(auto_advance, TimeFlow::Forward);
    }

    fn get_next_auto_advance_duration(&self, hint: Duration) -> Duration {
        let before = self.auto_advance_total;
        let next = self.auto_advance_total.saturating_add(hint);

        self.auto_advance_total_max
            .map_or(hint, |max| next.min(max).saturating_sub(before))
    }

    fn advance(&mut self, duration: Duration, flow: TimeFlow) {
        self.advance_time(duration, flow);
        self.evaluate_timers();
    }

    fn evaluate_timers(&mut self) {
        if self.auto_advance_timers {
            if let Some(last_timer) = self.timers.last_timer() {
                // we need to respect max auto-advance
                self.auto_advance(Some(
                    last_timer.tick().saturating_duration_since(self.instant),
                ));
            }
        } else {
            self.timers.advance_timers(self.instant);
        }
    }

    fn advance_time(&mut self, duration: Duration, flow: TimeFlow) {
        if duration == Duration::ZERO {
            return;
        }

        match flow {
            TimeFlow::Forward => {
                self.instant = self
                    .instant
                    .checked_add(duration)
                    .expect(OUTSIDE_RANGE_MESSAGE);
                self.timestamp = self
                    .timestamp
                    .checked_add(duration)
                    .expect(OUTSIDE_RANGE_MESSAGE);
                self.timers.advance_timers(self.instant);
            }
            TimeFlow::Backward => {
                self.instant = self
                    .instant
                    .checked_sub(duration)
                    .expect(OUTSIDE_RANGE_MESSAGE);
                self.timestamp = self
                    .timestamp
                    .checked_sub(duration)
                    .expect(OUTSIDE_RANGE_MESSAGE);

                // There is no point of advancing/triggering the timers if we are moving back
                // in time. Timers are only ever fired when time moves forward.
                // No need to call `self.timers.advance_timers` here.
            }
        }
    }

    fn now(&mut self) -> Timestamp {
        let time = self.timestamp;
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

static OUTSIDE_RANGE_MESSAGE: &str = "moving the clock outside of the supported time range is not possible: [1970-01-01T00:00:00Z, 9999-12-30T22:00:00.9999999Z]";

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use futures::task::noop_waker;

    use super::*;
    use crate::{Clock, Stopwatch};

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
        assert_eq!(
            control.now(),
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH).unwrap()
        );
    }

    #[test]
    fn auto_advance_ok() {
        let duration = Duration::from_secs(1);
        let control = ClockControl::new().auto_advance(duration);
        let clock = Clock::with_control(&control);

        assert_eq!(control.with_state(|s| s.auto_advance), duration);
        let now = clock.now();
        assert_eq!(clock.now().checked_duration_since(now).unwrap(), duration);

        let watch = Stopwatch::with_clock(&clock);
        assert_eq!(watch.elapsed(), duration);
    }

    #[test]
    fn advance_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        let now = clock.now();

        // act
        control.advance(Duration::from_secs(1));

        // assert
        assert_eq!(
            clock.now().checked_duration_since(now).unwrap(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn advance_to_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        let now = clock.now();

        // act
        control.advance_to(now.checked_add(Duration::from_secs(1)).unwrap());

        // assert
        assert_eq!(
            clock.now().checked_duration_since(now).unwrap(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn advance_to_past_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        let now = clock.now();

        // act
        control.advance_to(now.checked_add(Duration::from_secs(10)).unwrap());
        let now1 = clock.now();
        let instant_now1 = clock.instant_now();

        control.advance_to(now1.checked_sub(Duration::from_secs(5)).unwrap());
        let now2 = clock.now();
        let instant_now2 = clock.instant_now();

        // assert
        assert_eq!(
            now1.checked_duration_since(now2).unwrap(),
            Duration::from_secs(5)
        );

        assert_eq!(
            instant_now1.checked_duration_since(instant_now2).unwrap(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn advance_millis_ok() {
        // arrange
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        let now = clock.now();

        // act
        control.advance_millis(123);

        // assert
        assert_eq!(
            clock.now().checked_duration_since(now).unwrap(),
            Duration::from_millis(123)
        );
    }

    #[test]
    fn register_timer_ok() {
        // arrange
        let control = ClockControl::new();

        // act
        let key = control.register_timer(Instant::now(), noop_waker());

        // assert
        assert_eq!(control.timers_len(), 1);
        control.unregister_timer(key);
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn next_timer_ok() {
        let control = ClockControl::new();

        assert_eq!(control.next_timer(), None);

        let key = control.register_timer(Instant::now(), noop_waker());
        assert_eq!(control.next_timer().unwrap(), key.tick());
    }

    #[test]
    fn unregister_timer_ok() {
        // arrange
        let control = ClockControl::new();
        let key = control.register_timer(Instant::now(), noop_waker());

        // act
        control.unregister_timer(key);

        // assert
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn auto_advance_timers() {
        let control = ClockControl::new().auto_advance_timers(true);
        let clock = Clock::with_control(&control);
        let now = clock.now();

        control.register_timer(clock.instant_now() + Duration::from_secs(100), noop_waker());

        // assert
        assert_eq!(
            clock.now().checked_duration_since(now).unwrap(),
            Duration::from_secs(100)
        );
    }

    #[test]
    fn advance_ensure_timers_advanced() {
        // arrange
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        control.register_timer(clock.instant_now() + Duration::from_secs(1), noop_waker());

        // act
        control.advance(Duration::from_secs(1));

        // assert
        assert_eq!(control.timers_len(), 0);
    }

    #[test]
    fn auto_advance_with_max() {
        let control = ClockControl::new()
            .auto_advance_with_max(Duration::from_millis(550), Duration::from_secs(2));
        let clock = Clock::with_control(&control);

        let anchor = clock.now();

        assert_eq!(
            clock.now().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(550)
        );

        assert_eq!(
            clock.now().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(1100)
        );

        assert_eq!(
            clock.now().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(1650)
        );

        assert_eq!(
            clock.now().checked_duration_since(anchor).unwrap(),
            Duration::from_millis(2000)
        );

        assert_eq!(
            clock.now().checked_duration_since(anchor).unwrap(),
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
}