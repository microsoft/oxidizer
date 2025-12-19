// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::Clock;
use crate::runtime::clock_driver::ClockDriver;
use crate::state::GlobalState;

/// An inactive clock that must be activated before time operations can be performed.
///
/// This type represents a clock in an inactive state that cannot perform any time-related
/// operations until activated. It can be safely cloned and moved across threads, making it
/// suitable for initialization in multi-threaded environments.
///
/// To begin using the clock, call [`InactiveClock::activate`] to get a working [`Clock`] instance and
/// its associated [`ClockDriver`]. The driver is responsible for advancing timers and must
/// be called periodically by the runtime.
///
/// # Examples
///
/// ```rust
/// use tick::runtime::InactiveClock;
///
/// let inactive = InactiveClock::default();
/// let (clock, driver) = inactive.activate();
///
/// // Use the clock for time operations
/// let now = clock.instant();
///
/// // Driver must be advanced periodically (typically by the runtime)
/// // driver.advance_timers();
/// ```
///
/// # Thread-per-core runtimes
///
/// Thread-per-core runtimes can activate separate clock instances per thread by cloning
/// the [`InactiveClock`] before activation. This eliminates lock contention and improves
/// performance.
#[derive(Debug, Clone, Default)]
pub struct InactiveClock(GlobalState);

impl InactiveClock {
    /// Activates the clock for time operations.
    ///
    /// Consumes this inactive clock and returns a working [`Clock`] instance along with
    /// its [`ClockDriver`]. The driver must be called periodically to advance timers.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - [`Clock`] - The activated clock instance for time operations
    /// - [`ClockDriver`] - Driver that advances timers (must be polled by caller)
    #[must_use]
    pub fn activate(self) -> (Clock, ClockDriver) {
        let state = self.0.into_clock_state();

        let clock = Clock(Arc::clone(&state));
        let driver = ClockDriver::new(state);

        (clock, driver)
    }
}

#[cfg(any(feature = "test-util", test))]
impl From<crate::ClockControl> for InactiveClock {
    fn from(control: crate::ClockControl) -> Self {
        Self(GlobalState::ClockControl(control))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClockControl, state::ClockState};

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(InactiveClock: Send, Sync);
    }

    #[test]
    fn activate_ok() {
        let inactive_clock = InactiveClock::default();
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::System(_)));
        assert!(matches!(driver.0.as_ref(), &ClockState::System(_)));
    }

    #[test]
    fn activate_with_fake_clock_ok() {
        let inactive_clock = InactiveClock::from(ClockControl::new());
        let (clock, driver) = inactive_clock.activate();
        assert!(matches!(clock.clock_state(), ClockState::ClockControl(_)));
        assert!(matches!(driver.0.as_ref(), &ClockState::ClockControl(_)));
    }
}
