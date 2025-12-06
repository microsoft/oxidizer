// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

use crate::state::ClockState;

/// Drives timer advancement for the clock system.
///
/// The `ClockDriver` is responsible for advancing and firing timers associated with
/// the clock. The runtime must call [`ClockDriver::advance_timers`] periodically
/// to ensure timers are processed correctly.
#[derive(Debug)]
pub struct ClockDriver(pub(super) ClockState);

impl ClockDriver {
    pub(super) const fn new(state: ClockState) -> Self {
        Self(state)
    }

    /// Advances and fires timers that should execute by the given time.
    ///
    /// This method processes all timers that are scheduled to fire at or before
    /// the specified `now` time, executing their associated wakers.
    ///
    /// # Arguments
    ///
    /// * `now` - The current time to advance timers to
    ///
    /// # Returns
    ///
    /// Returns `Some(instant)` with the next scheduled timer time if any timers
    /// are registered, or `None` if no timers are pending.
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    #[must_use]
    pub fn advance_timers(&self, now: Instant) -> Option<Instant> {
        match self.0 {
            ClockState::System(ref timers) => timers.try_advance_timers(now),
            #[cfg(any(feature = "test-util", test))]
            ClockState::ClockControl(ref c) => c.next_timer(),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::task::Waker;
    use std::time::Duration;

    use super::*;
    use crate::state::SynchronizedTimers;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(ClockDriver: Send, Sync);
    }

    #[test]
    fn advance_timers_ok() {
        let timers = SynchronizedTimers::default();
        let clock_state = ClockState::System(timers.clone());
        let when = Instant::now();

        timers.with_timers(|timers| {
            timers.register(when, Waker::noop().clone());
        });

        let driver = ClockDriver::new(clock_state);

        _ = driver.advance_timers(Instant::now() - Duration::from_secs(1));
        timers.with_timers(|timers| assert_eq!(timers.len(), 1));

        _ = driver.advance_timers(Instant::now() + Duration::from_secs(1));
        timers.with_timers(|timers| assert_eq!(timers.len(), 0));
    }
}
