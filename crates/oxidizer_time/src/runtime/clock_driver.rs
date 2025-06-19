// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;

use crate::state::ClockState;

/// A drivers that is used to advance the timers associated with the  clock.
///
/// Runtime must make sure that [`ClockDriver::advance_timers`] is called periodically
/// to advance the timers.
#[derive(Debug)]
pub struct ClockDriver(pub(super) ClockState);

impl ClockDriver {
    pub(super) const fn new(state: ClockState) -> Self {
        Self(state)
    }

    /// Advances and fires all the timers are should be fired up until the `now`.
    ///
    /// Returns the next time when the timer should be fired. If no timers are registered, returns `None`.
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    #[must_use]
    pub fn advance_timers(&self, now: Instant) -> Option<Instant> {
        match self.0 {
            ClockState::System(ref timers) => timers.try_advance_timers(now),
            #[cfg(any(feature = "fakes", test))]
            ClockState::ClockControl(ref c) => c.next_timer(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::task::noop_waker;

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
            timers.register(when, noop_waker());
        });

        let driver = ClockDriver::new(clock_state);

        _ = driver.advance_timers(Instant::now() - Duration::from_secs(1));
        timers.with_timers(|timers| assert_eq!(timers.len(), 1));

        _ = driver.advance_timers(Instant::now() + Duration::from_secs(1));
        timers.with_timers(|timers| assert_eq!(timers.len(), 0));
    }
}