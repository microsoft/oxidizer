// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

use crate::runtime::ClockGone;
use crate::state::{ClockState, DriverRegistration};

/// Drives timer advancement for the clock.
///
/// The `ClockDriver` is responsible for advancing and firing timers associated with
/// the clock. The runtime advances it, parks until the returned deadline, and uses the waker passed
/// to [`InactiveClock::activate`](crate::runtime::InactiveClock::activate) to interrupt that wait
/// when an earlier timer is registered.
///
/// In thread-isolated runtime architectures using one coordinate owner, each
/// worker owns its own `ClockDriver` that advances only the timers registered
/// on that thread's [`Clock`][crate::Clock]. See the [`runtime`][crate::runtime]
/// module for the setup pattern.
///
/// Controlled clocks ignore the waker supplied to
/// [`InactiveClock::activate`](crate::runtime::InactiveClock::activate). Their external owner must
/// advance controlled time and explicitly wake, exit, and drop every driver loop during lifecycle
/// shutdown; controlled drivers must not wait solely for [`ClockGone`].
#[derive(Debug)]
pub struct ClockDriver {
    pub(crate) state: ClockState,
    driver_id: DriverRegistration,
}

impl ClockDriver {
    pub(super) const fn new(state: ClockState, driver_id: DriverRegistration) -> Self {
        Self { state, driver_id }
    }

    /// Advances and fires all timers scheduled to execute by the given time.
    ///
    /// This method processes all timers scheduled to fire at or before the
    /// specified `now` time, waking their associated tasks.
    ///
    /// Returns the next system-clock deadline suitable for parking. Controlled clocks return
    /// `None` because their time and timer wakeups are driven externally by their owner.
    ///
    /// # Errors
    ///
    /// Returns `Err(ClockGone)` if the all clocks are gone, all timers are fired and
    /// advancing the clock is no longer necessary.
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "the mut forces exclusive ownership of the driver")]
    pub fn advance_timers(&mut self, now: Instant) -> Result<Option<Instant>, ClockGone> {
        let next_timer = match &self.state {
            ClockState::System(storage) => storage.try_advance_timers(now),
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(_) => None,
        };

        match next_timer {
            Some(next) => Ok(Some(next)),
            None if self.state.driver_is_finished() => Err(ClockGone::new()),
            None => Ok(None),
        }
    }
}

impl Drop for ClockDriver {
    fn drop(&mut self) {
        self.state.unregister_driver(self.driver_id);
    }
}
