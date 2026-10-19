// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

use crate::runtime::ClockGone;
use crate::state::{ClockState, DriverId};

/// Drives timer advancement for the clock.
///
/// The `ClockDriver` is responsible for advancing and firing timers associated with
/// the clock. The runtime advances it and parks until the returned deadline. Drivers created by
/// [`InactiveClock::activate_with_driver_waker`](crate::runtime::InactiveClock::activate_with_driver_waker)
/// can also interrupt that wait when an earlier timer is registered.
///
/// In thread-isolated runtime architectures using one coordinate owner, each
/// worker owns its own `ClockDriver` that advances only the timers registered
/// on that thread's [`Clock`][crate::Clock]. See the [`runtime`][crate::runtime]
/// module for the setup pattern.
#[derive(Debug)]
pub struct ClockDriver {
    pub(crate) state: ClockState,
    driver_id: Option<DriverId>,
}

impl ClockDriver {
    pub(super) const fn new(state: ClockState, driver_id: Option<DriverId>) -> Self {
        Self { state, driver_id }
    }

    /// Returns whether the runtime must retain bounded periodic polling between driver iterations.
    ///
    /// This is `false` only for a system clock activated with
    /// [`InactiveClock::activate_with_driver_waker`](crate::runtime::InactiveClock::activate_with_driver_waker).
    /// Those drivers receive first-timer, newly earlier-timer, and final-owner notifications and
    /// may park indefinitely when [`Self::advance_timers`] returns `Ok(None)`.
    ///
    /// Compatibility drivers created by [`InactiveClock::activate`](crate::runtime::InactiveClock::activate)
    /// return `true` because timer registration cannot interrupt either a no-deadline wait or a
    /// wait for an existing later deadline. Externally advanced controlled clocks also return
    /// `true`.
    #[must_use]
    pub const fn requires_periodic_poll(&self) -> bool {
        self.driver_id.is_none()
    }

    /// Advances and fires all timers scheduled to execute by the given time.
    ///
    /// This method processes all timers scheduled to fire at or before the
    /// specified `now` time, waking their associated tasks.
    ///
    /// Returns the next system-clock deadline suitable for parking. Controlled clocks return
    /// `None` because their time advances externally rather than with [`Instant`]; runtimes using
    /// controlled clocks must retain a periodic poll so external-owner drops are eventually
    /// observed.
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
        self.state.unregister_driver(self.driver_id.take());
    }
}
