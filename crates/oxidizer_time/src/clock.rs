// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
#[cfg(any(feature = "tokio", test))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Waker;
use std::time::Instant;

#[cfg(any(feature = "fakes", test))]
use super::clock_control::ClockControl;
use super::{TimerKey, Timestamp};
use crate::state::ClockState;

/// Clock provides an abstraction for time-related operations.
///
/// Working with time is notoriously difficult to test and control. The clock provides a way to control the flow of time.
/// When used in production, clock offers almost zero overhead over using the system-related time APIs directly. However,
/// when used in tests, the clock provides additional functionality to control the flow of time. This makes the tests faster
/// and more reliable. See the [Testing](#testing) section for more information.
///
/// The clock is used for:
///
/// - Retrieving the current UTC timestamp as [`Timestamp`].
/// - Creation of [`Stopwatch`][super::Stopwatch] that
///   simplifies the time measurements and can be used as the relative unit of time.
/// - Clock is also used when for creation of [`PeriodicTimer`][super::PeriodicTimer] and [`Delay`][super::Delay].
///
/// # Relative and absolute time
///
/// The clock provides two types of time representation:
///
/// - [`Stopwatch`][super::Stopwatch]: Representation of relative time that is monotonic. This is useful for measuring
///   the elapsed time. The use of relative time is recommended when a point in time does not cross the process boundaries.
/// - [`Timestamp`]: Represents an absolute point in time. The timestamp is not monotonic and can be affected by the
///   system clock changes. Use timestamp when you need to represent an absolute point in time that crosses process
///   boundaries. The timestamp supports formatting and parsing operations, serialization and deserialization or manual creation.
///
/// When possible, always prefer [`Stopwatch`][super::Stopwatch] over [`Timestamp`] due to its monotonic properties.
/// For scenarios, where you need share the time outside the process boundaries, the [`Timestamp`] is the only option.
///
/// # Clock construction
///
/// The clock requires runtime to drive the registered timers. For this reason, the clock construction is non-trivial
/// and clock access is provided by the runtime.
///
/// In tests, the clock can be constructed directly because the flow of time is controlled manually.
/// See the [Testing](#testing) section for more information.
///
/// # Testing
///
/// When working with time, it's difficult to isolate the time-related operations in tests. Typical example is the sleep
/// operation, that is hard to test and slows down the tests. What you want to do is to have a complete control over the flow
/// of time that allows you to jump forward in time. This is where the clock comes into play.
///
/// The ability to jump forward in time makes the tests faster, more reliable and gives you complete control over the flow of time.
/// By default, the clock does not allow you to control the flow of time. However, when the `fakes` feature is enabled, Oxidizer
/// provides a `ClockControl` type that can be used to control the time.
///
/// # State sharing between clocks
///
/// The clock does not have any internal state when running in production. However, when running in tests,
/// multiple clock instances can be linked together and share the same state, that is the flow of time.
///
/// To ensure the state sharing between clock, clone the clock. Cloning operation preserves the shared state
/// between individual clocks. The clone operation is also extremely cheap when running in production.
///
/// ```
/// use oxidizer_time::Clock;
///
/// fn use_clock(clock: &Clock) {
///     // Cloned clocks, all these instances are linked
///     // together and share the same state.
///     let clock_clone1 = clock.clone();
///     let clock_clone2 = clock.clone();
/// }
///
/// # use_clock(&Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance(std::time::Duration::from_secs(1))));
/// ```
///
/// # Examples
///
/// ### Retrieve UTC timestamp
///
/// ```
/// use oxidizer_time::Clock;
///
/// fn retrieve_timestamp(clock: &Clock) {
///     let timestamp1 = clock.now();
///     let timestamp2 = clock.now();
///
///     assert!(timestamp2 >= timestamp1);
/// }
///
/// # retrieve_timestamp(&Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance(std::time::Duration::from_secs(1))));
/// ```
///
/// ### Measure the elapsed time
///
/// ```
/// use std::time::Duration;
/// use oxidizer_time::{Clock, Stopwatch};
///
/// fn measure(clock: &Clock) {
///     let stopwatch = Stopwatch::with_clock(&clock);
///     // Perform some operation...
///     let elapsed: Duration = stopwatch.elapsed();
/// }
/// # measure(&Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance(Duration::from_secs(1))));
/// ```
///
/// ### Delay operations
///
/// ```
/// use oxidizer_time::{Clock, Stopwatch, Delay};
/// use std::time::Duration;
///
/// async fn delay_example(clock: &Clock) {
///     let stopwatch = Stopwatch::with_clock(&clock);
///
///     // Delay for 10 millis
///     Delay::with_clock(&clock, Duration::from_millis(10)).await;
///
///     assert!(stopwatch.elapsed() >= Duration::from_millis(10));
/// }
///
/// # fn main() {
/// #     let clock = Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance_timers(true));
/// #     futures::executor::block_on(delay_example(&clock));
/// # }
/// ```
///
/// ### Create periodic timers
///
/// ```
/// use oxidizer_time::{Clock, PeriodicTimer};
/// use std::time::Duration;
/// use futures::StreamExt;
///
/// async fn timer_example(clock: &Clock) {
///     let mut timer = PeriodicTimer::with_clock(&clock, Duration::from_millis(10));
///
///     while let Some(()) = timer.next().await {
///         // do something
///         # break;
///     }
/// }
///
/// # fn main() {
/// #     let clock = Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance_timers(true));
/// #     futures::executor::block_on(timer_example(&clock));
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Clock(Arc<ClockInner>);

impl Clock {
    /// Creates a new clock that is driven by the tokio runtime.
    ///
    /// The resolution of the clock is set to `10` milliseconds.
    #[cfg(any(feature = "tokio", test))]
    #[must_use]
    pub fn tokio() -> Self {
        Self::tokio_core().0
    }

    #[cfg(any(feature = "tokio", test))]
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    fn tokio_core() -> (Self, tokio::task::JoinHandle<()>) {
        use std::time::Duration;

        use crate::runtime::InactiveClock;

        const TIMER_RESOLUTION: Duration = Duration::from_millis(10);

        let cancellation = Arc::new(AtomicBool::new(false));
        let cancellation_clone = Arc::clone(&cancellation);
        let (state, driver) = InactiveClock::default().activate_with_state();

        // Spawn a task that advances the timers.
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TIMER_RESOLUTION).await;

                // Stops the loop when there are no more timers and the
                // clock is gone. (indicated by the cancellation flag)
                if driver.advance_timers(Instant::now()).is_none()
                    && cancellation_clone.load(Ordering::Relaxed)
                {
                    break;
                }
            }
        });

        (
            Self(Arc::new(ClockInner::Tokio {
                state,
                cancellation,
            })),
            join_handle,
        )
    }

    /// Used for testing. For this clock, the timers are not moving forward.
    #[cfg(test)]
    pub(super) fn new_dormant() -> Self {
        Self::with_state(crate::state::GlobalState::System.into())
    }

    pub(super) fn with_state(state: ClockState) -> Self {
        Self(Arc::new(ClockInner::State(state)))
    }

    #[cfg(any(feature = "fakes", test))]
    #[must_use]
    pub fn with_control(clock_control: &ClockControl) -> Self {
        Self::with_state(ClockState::ClockControl(clock_control.clone()))
    }

    /// Retrieves the current [`Timestamp`].
    ///
    /// The `Timestamp` represents the number of elapsed nanoseconds since the [`SystemTime::UNIX_EPOCH`][std::time::SystemTime::UNIX_EPOCH].
    /// The timestamp retrieved from the clock is not monotonic and can be affected by the system clock changes. It's possible,
    /// when the system clock changes, that the current timestamp is older than previously retrieved one.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidizer_time::Clock;
    ///
    /// fn retrieve_timestamp(clock: &Clock) {
    ///     let timestamp1 = clock.now();
    ///     let timestamp2 = clock.now();
    ///
    ///     assert!(timestamp2 >= timestamp1);
    /// }
    ///
    /// # retrieve_timestamp(&Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance(std::time::Duration::from_secs(1))));
    /// ```
    #[must_use]
    pub fn now(&self) -> Timestamp {
        match self.clock_state() {
            #[cfg(any(feature = "fakes", test))]
            ClockState::ClockControl(control) => control.now(),
            ClockState::System(_) => Timestamp::now(),
        }
    }

    /// Retrieves the current [`Instant`] time.
    pub(super) fn instant_now(&self) -> Instant {
        match self.clock_state() {
            #[cfg(any(feature = "fakes", test))]
            ClockState::ClockControl(control) => control.instant_now(),
            ClockState::System(_) => Instant::now(),
        }
    }

    pub(super) fn register_timer(&self, when: Instant, waker: Waker) -> TimerKey {
        match self.clock_state() {
            #[cfg(any(feature = "fakes", test))]
            ClockState::ClockControl(control) => control.register_timer(when, waker),
            ClockState::System(timers) => timers.with_timers(|t| t.register(when, waker)),
        }
    }

    pub(super) fn unregister_timer(&self, key: TimerKey) {
        match self.clock_state() {
            #[cfg(any(feature = "fakes", test))]
            ClockState::ClockControl(control) => control.unregister_timer(key),
            ClockState::System(timers) => timers.with_timers(|t| t.unregister(key)),
        }
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub(crate) fn clock_state(&self) -> &ClockState {
        self.0.local_state()
    }
}

#[derive(Debug)]
enum ClockInner {
    State(ClockState),

    #[cfg(any(feature = "tokio", test))]
    Tokio {
        state: ClockState,
        cancellation: Arc<AtomicBool>,
    },
}

impl ClockInner {
    const fn local_state(&self) -> &ClockState {
        match self {
            Self::State(state) => state,
            #[cfg(any(feature = "tokio", test))]
            Self::Tokio { state, .. } => state,
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Causes test timeout.
impl Drop for ClockInner {
    fn drop(&mut self) {
        match self {
            Self::State(_) => {}
            #[cfg(any(feature = "tokio", test))]
            Self::Tokio { cancellation, .. } => {
                cancellation.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::arithmetic_side_effects,
        reason = "no need to be strict in tests"
    )]

    use std::time::Duration;

    use futures::task::noop_waker;

    use super::*;
    use crate::Delay;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Clock: Send, Sync);
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn test_now() {
        let now = std::time::SystemTime::now();

        let clock = Clock::new_dormant();
        let absolute = clock.now();
        assert!(absolute.to_system_time() >= now);
    }

    #[test]
    fn test_now_with_control() {
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);

        let now = clock.now();
        assert_eq!(now, control.now());

        control.advance(Duration::from_secs(10));

        assert_eq!(
            clock.now(),
            now.checked_add(Duration::from_secs(10)).unwrap()
        );
    }

    #[test]
    fn test_instant_now() {
        let clock = Clock::new_dormant();
        let clock_instant = clock.instant_now();
        let system_instant = Instant::now();

        assert!(
            (system_instant - clock_instant) < Duration::from_secs(10),
            "the `Instant` retrieved from the clock is not the same as system one"
        );
    }

    #[test]
    fn register_timer() {
        let clock = Clock::new_dormant();
        let id1 = clock.register_timer(Instant::now(), noop_waker());
        let id2 = clock.register_timer(Instant::now(), noop_waker());

        assert_ne!(id1, id2);
    }

    #[cfg(not(miri))] // The logic we call talks to the real OS, which Miri cannot do.
    #[tokio::test]
    async fn tokio_ensure_timers_advancing() {
        let clock = Clock::tokio();
        Delay::with_clock(&clock, Duration::from_millis(15)).await;
    }

    #[cfg(not(miri))] // The logic we call talks to the real OS, which Miri cannot do.
    #[tokio::test]
    async fn tokio_ensure_future_finished_when_clock_dropped() {
        let (clock, handle) = Clock::tokio_core();

        Delay::with_clock(&clock, Duration::from_millis(15)).await;

        drop(clock);

        handle.await.unwrap();
    }
}