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
/// This is the default mode. If coordinates share one runtime owner,
/// [`ThreadAware::relocate`] gives each destination thread separate timer
/// storage. Use this mode for runtimes with stable worker threads.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct Isolated;

/// Marker for an [`InactiveClock`] backed by a single shared timer set.
///
/// [`InactiveClock::new_shared`] creates this mode. `InactiveClock<Shared>` does
/// not implement [`Clone`] or [`ThreadAware`]. One driver must advance the one
/// shared timer set.
#[derive(Debug)]
#[non_exhaustive]
pub struct Shared;

/// A clock that must be activated before use.
///
/// The mode marker `S` selects the timer-storage model:
///
/// - [`Isolated`] is the default. Each relocated thread gets a separate timer
///   set after activation.
/// - [`Shared`]: a single shared timer set advanced by a single driver. Use
///   [`InactiveClock::new_shared`] to create it. This mode does not implement
///   [`Clone`] or [`ThreadAware`].
///
/// Call [`InactiveClock::activate`] with the runtime driver waker. It returns a
/// [`Clock`] and its [`ClockDriver`]. For system clocks, the first timer and each
/// newly earlier timer invoke the waker. This interrupts the current driver
/// wait. Controlled clocks ignore the waker. See
/// [controlled clocks](crate::runtime#controlled-clocks).
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
    /// Creates an [`InactiveClock`] with one shared timer set.
    ///
    /// The returned value does not implement [`Clone`] or [`ThreadAware`].
    /// Exactly one [`ClockDriver`] must advance its timer set.
    /// [`Clock::new_tokio`][crate::Clock::new_tokio] uses this mode.
    #[must_use]
    pub fn new_shared() -> Self {
        Self {
            state: ClockState::new_system_shared(),
            _marker: PhantomData,
        }
    }
}

impl<S> InactiveClock<S> {
    /// Activates the clock.
    ///
    /// This method consumes the inactive clock and returns a [`Clock`] with its
    /// [`ClockDriver`]. These events invoke `driver_waker`:
    ///
    /// - The first system timer is registered.
    /// - A new timer has an earlier deadline.
    /// - The final clock owner is dropped.
    ///
    /// Controlled clocks do not register the driver waker. Their owner advances
    /// time and wakes ready timers. The owner must also wake each driver loop
    /// during shutdown or owner-lifecycle changes. The loop must stop from this
    /// external state instead of waiting only for
    /// [`ClockGone`](crate::runtime::ClockGone).
    ///
    /// A driver notification can occur after
    /// [`ClockDriver::advance_timers`] returns but before the runtime parks.
    /// The runtime must retain pending wakes or make this transition race-free.
    /// Otherwise, it can lose a timer registration.
    ///
    /// The retained waker must not own a related [`Clock`] or [`ClockDriver`],
    /// including through another value. Such ownership creates a cycle that can
    /// prevent final cleanup. Use a separate signal or a weak runtime handle.
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
