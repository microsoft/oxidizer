// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Clock;
use crate::runtime::clock_driver::ClockDriver;
use crate::state::{ClockState, GlobalState};

/// A clock that is not yet active and cannot be used for any time-related operations.
///
/// This type can be cloned and moved across threads. To activate the clock and start using it,
/// the [`InactiveClock::activate`] method shall be called that returns a [`Clock`] and a [`ClockDriver`].
///
/// The caller that actives the clock is responsible for moving the timers forward. This is done by calling
/// the [`ClockDriver::advance_timers`] method periodically.
///
/// # Single-threaded runtimes
///
/// Single threaded can activate the clock for each thread separately. This can be done by cloning the
/// [`InactiveClock`] and moving it to a thread where it is activated. This allows each thread
/// to avoid the lock contention and get better performance.
#[derive(Debug, Clone, Default)]
pub struct InactiveClock(GlobalState);

impl InactiveClock {
    /// Activates the clock, returning a [`Clock`] and a [`ClockDriver`] that moves the timers forward.
    #[must_use]
    pub fn activate(self) -> (Clock, ClockDriver) {
        let (state, driver) = self.activate_with_state();

        (Clock::with_state(state), driver)
    }

    pub(crate) fn activate_with_state(self) -> (ClockState, ClockDriver) {
        let state = ClockState::from(self.0);
        (state.clone(), ClockDriver::new(state))
    }
}

#[cfg(any(feature = "fakes", test))]
impl From<crate::ClockControl> for InactiveClock {
    fn from(control: crate::ClockControl) -> Self {
        Self(GlobalState::ClockControl(control))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClockControl;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(InactiveClock: Send, Sync);
    }

    #[test]
    fn activate_ok() {
        let inactive_clock = InactiveClock::default();
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::System(_)));
        assert!(matches!(driver.0, ClockState::System(_)));
    }

    #[test]
    fn activate_with_fake_clock_ok() {
        let inactive_clock = InactiveClock::from(ClockControl::new());
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::ClockControl(_)));
        assert!(matches!(driver.0, ClockState::ClockControl(_)));
    }
}