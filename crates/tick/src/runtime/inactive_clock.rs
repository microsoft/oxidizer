// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::task::Waker;

use thread_aware::{Thread, ThreadAware};

use crate::Clock;
use crate::runtime::clock_driver::ClockDriver;
use crate::state::ClockState;

/// Marker for an [`InactiveClock`] backed by per-thread isolated timer storage.
///
/// This is the default mode. Clones can be relocated to different threads via
/// [`ThreadAware::relocate`], producing independent timer storage per thread
/// when the coordinates share one runtime owner. Suitable for runtimes with
/// stable worker threads.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct Isolated;

/// Marker for an [`InactiveClock`] backed by a single shared timer set.
///
/// Created via [`InactiveClock::new_shared`]. The resulting `InactiveClock<Shared>`
/// intentionally does **not** implement [`Clone`] or [`ThreadAware`]: there is exactly one
/// shared timer set advanced by exactly one driver, so cloning or relocation would create
/// configurations the driver could not advance correctly.
#[derive(Debug)]
#[non_exhaustive]
pub struct Shared;

/// An inactive clock that must be activated before time operations can be performed.
///
/// This type represents a clock in an inactive state that cannot perform any time-related
/// operations until activated. It is parameterized by a mode marker (`S`):
///
/// - [`Isolated`] (default): per-thread timer storage. The inactive clock can be cloned and
///   relocated across threads, with each thread getting an independent timer set on
///   activation.
/// - [`Shared`]: a single shared timer set advanced by a single driver. Use
///   [`InactiveClock::new_shared`] to construct one. Does not implement [`Clone`] or
///   [`ThreadAware`].
///
/// To begin using the clock, call [`InactiveClock::activate`] to get a working [`Clock`] instance and
/// its associated [`ClockDriver`]. This compatibility path requires bounded periodic polling
/// between every driver iteration, including while a deadline is pending. Notification-aware
/// runtimes can instead call
/// [`InactiveClock::activate_with_driver_waker`] so first and newly earlier timer registrations can
/// interrupt the driver's current wait.
///
/// # Examples
///
/// ```no_run
/// use std::thread;
/// use std::time::Instant;
///
/// use tick::runtime::InactiveClock;
///
/// let inactive = InactiveClock::default();
/// let (clock, mut driver) = inactive.activate();
///
/// // Use the clock for time operations
/// let now = clock.instant();
///
/// // A real runtime loop advances the driver, then parks until the deadline or until
/// // a bounded fallback because this compatibility path does not receive registration
/// // notifications.
/// loop {
///     match driver.advance_timers(Instant::now()) {
///         Ok(Some(deadline)) => thread::park_timeout(
///             deadline
///                 .saturating_duration_since(Instant::now())
///                 .min(std::time::Duration::from_millis(10)),
///         ),
///         Ok(None) => thread::park_timeout(std::time::Duration::from_millis(10)),
///         Err(_) => break,
///     }
/// }
/// ```
///
/// # Thread-aware isolated runtimes
///
/// In runtimes that keep work on stable worker threads, clone the `InactiveClock` and
/// [`relocate`](thread_aware::ThreadAware::relocate) each clone to its target thread before
/// activation. Relocation creates per-thread timer storage, so each thread gets an independent set
/// of timers with no cross-thread lock contention. Construct those coordinates from clones of one
/// [`ThreadBuilder`](thread_aware::ThreadBuilder), so they share a runtime owner. A
/// cross-owner relocation retains the existing timer set and remains functional, but does not
/// establish destination-local storage.
#[derive(Debug)]
pub struct InactiveClock<S = Isolated> {
    state: ClockState,
    _marker: PhantomData<fn() -> S>,
}

impl Clone for InactiveClock<Isolated> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            _marker: PhantomData,
        }
    }
}

impl Default for InactiveClock<Isolated> {
    fn default() -> Self {
        Self {
            state: ClockState::new_system(),
            _marker: PhantomData,
        }
    }
}

impl ThreadAware for InactiveClock<Isolated> {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.state.relocate(source, destination);
    }
}

#[cfg(any(feature = "rt-shared", test))]
impl InactiveClock<Shared> {
    /// Creates an [`InactiveClock`] backed by a single shared timer set.
    ///
    /// The returned value is intentionally not [`Clone`] and does not implement
    /// [`ThreadAware`]: a `Shared` clock has exactly one timer set that must be advanced by
    /// exactly one [`ClockDriver`]. This is the construction mode used by
    /// [`Clock::new_tokio`][crate::Clock::new_tokio].
    #[must_use]
    pub fn new_shared() -> Self {
        Self {
            state: ClockState::new_system_shared(),
            _marker: PhantomData,
        }
    }
}

impl<S> InactiveClock<S> {
    /// Activates the clock for time operations.
    ///
    /// Consumes this inactive clock and returns a working [`Clock`] instance along with
    /// its [`ClockDriver`]. This source-compatible activation path does not receive timer
    /// registration notifications, so the driver requires bounded periodic polling between every
    /// driver iteration, including while waiting for a returned deadline. Without that bound, a
    /// newly registered earlier timer could not interrupt the existing wait.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - [`Clock`] - The activated clock instance for time operations
    /// - [`ClockDriver`] - Driver that advances timers (must be polled by caller)
    #[must_use]
    pub fn activate(self) -> (Clock, ClockDriver) {
        self.activate_inner(None)
    }

    /// Activates the clock with notifications for driver wait interruption.
    ///
    /// Registering the first system timer, a timer that precedes the current earliest deadline,
    /// or dropping the final clock owner invokes `driver_waker`. A system-clock driver can
    /// therefore park indefinitely when [`ClockDriver::advance_timers`] returns no deadline and
    /// [`ClockDriver::requires_periodic_poll`] is `false`.
    ///
    /// Controlled clocks do not register the waker because their time advances externally. Their
    /// drivers continue to require bounded periodic polling.
    ///
    /// The retained waker must not transitively own a [`Clock`] associated with this driver.
    /// Runtime loops commonly wait for [`ClockGone`](crate::runtime::ClockGone) before dropping the
    /// driver; a `Waker -> runtime state -> Clock` ownership cycle would keep the final clock owner
    /// alive and prevent that shutdown condition.
    #[must_use]
    pub fn activate_with_driver_waker(self, driver_waker: Waker) -> (Clock, ClockDriver) {
        self.activate_inner(Some(driver_waker))
    }

    fn activate_inner(self, driver_waker: Option<Waker>) -> (Clock, ClockDriver) {
        let driver_id = driver_waker.and_then(|driver_waker| self.state.register_driver(driver_waker));
        let state = self.state;
        let clock = Clock::new(state.clone());
        let driver = ClockDriver::new(state, driver_id);

        (clock, driver)
    }
}

#[cfg(any(feature = "test-util", test))]
impl From<crate::ClockControl> for InactiveClock<Isolated> {
    fn from(control: crate::ClockControl) -> Self {
        Self {
            state: ClockState::ClockControl(control),
            _marker: PhantomData,
        }
    }
}
