// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

use crate::runtime::ClockGone;
use crate::state::{ClockState, DriverRegistration};

/// Drives timer advancement for the clock.
///
/// The runtime advances the driver and parks until the returned deadline. If an
/// earlier timer is registered, the activation waker interrupts that wait.
///
/// In a thread-isolated runtime with one coordinate owner, each worker has one
/// driver. The driver advances only timers from that thread's
/// [`Clock`][crate::Clock]. See the [`runtime`][crate::runtime] module.
///
/// Controlled clocks ignore the activation waker. Their owner must advance
/// controlled time. During shutdown, the owner must wake, stop, and drop each
/// driver loop. A controlled driver must not wait only for [`ClockGone`].
#[derive(Debug)]
pub struct ClockDriver {
    pub(crate) state: ClockState,
    driver_id: DriverRegistration,
}

impl ClockDriver {
    pub(super) const fn new(state: ClockState, driver_id: DriverRegistration) -> Self {
        Self { state, driver_id }
    }

    /// Fires all timers scheduled at or before `now`.
    ///
    /// Returns the next system-clock deadline for runtime parking. A controlled
    /// clock returns `None` because its owner advances time and wakes timers.
    ///
    /// # Errors
    ///
    /// Returns `Err(ClockGone)` when all clocks are gone and no timer remains.
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
