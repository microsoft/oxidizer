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
/// its associated [`ClockDriver`]. Pass the runtime's driver waker so a newly registered earlier
/// timer can interrupt the current wait.
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use std::task::{Wake, Waker};
/// use std::thread;
/// use std::time::Instant;
///
/// use tick::runtime::InactiveClock;
///
/// struct DriverWake(thread::Thread);
///
/// impl Wake for DriverWake {
///     fn wake(self: Arc<Self>) {
///         self.0.unpark();
///     }
///
///     fn wake_by_ref(self: &Arc<Self>) {
///         self.0.unpark();
///     }
/// }
///
/// let inactive = InactiveClock::default();
/// let driver_waker = Waker::from(Arc::new(DriverWake(thread::current())));
/// let (clock, mut driver) = inactive.activate(driver_waker);
///
/// // Use the clock for time operations
/// let now = clock.instant();
///
/// // A real runtime loop advances the driver, then parks until the deadline or until
/// // registration of the first timer (or a newly earlier timer) invokes driver_waker.
/// loop {
///     match driver.advance_timers(Instant::now()) {
///         Ok(Some(deadline)) => {
///             thread::park_timeout(deadline.saturating_duration_since(Instant::now()))
///         }
///         Ok(None) => thread::park(),
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
    /// its [`ClockDriver`]. Registering the first system timer, or a timer that precedes the
    /// current earliest deadline, invokes `driver_waker`, allowing the runtime to interrupt its
    /// existing wait.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - [`Clock`] - The activated clock instance for time operations
    /// - [`ClockDriver`] - Driver that advances timers (must be polled by caller)
    #[must_use]
    pub fn activate(self, driver_waker: Waker) -> (Clock, ClockDriver) {
        let driver_id = self.state.register_driver(driver_waker);
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
