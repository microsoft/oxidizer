// Copyright (c) Microsoft Corporation.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::timers::Timers;

#[derive(Debug, Clone)]
pub enum ClockState {
    #[cfg(any(feature = "test-util", test))]
    ClockControl(crate::ClockControl),
    System(SynchronizedTimers),
}

impl ClockState {
    #[cfg(test)]
    pub(super) fn timers_len(&self) -> usize {
        match self {
            Self::ClockControl(control) => control.timers_len(),
            Self::System(timers) => timers.with_timers(|t| t.len()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SynchronizedTimers {
    // The mutex here is not accessed on a hot path. Timers are accessed only when:
    //
    // 1. A new timer is registered.
    // 2. A timer is unregistered.
    // 3. Timers are evaluated. Timer evaluation is very fast when there are no timers to fire. If
    //    there are timers to fire, the time to evaluate them is proportional to the number of timers
    //    that are ready to fire, and taking the lock is not the bottleneck.
    //
    // We have performed a [benchmark](https://o365exchange.visualstudio.com/O365%20Core/_git/ox-sdk?path=/crates/tick/benches/clock_bench.rs)
    // that compares the performance of this code by replacing the `Mutex`
    // with `RefCell`. The `RefCell` variant is around 7% faster. In practice, in real applications,
    // the difference is negligible. The real performance improvement comes from isolating the `Clock` to each thread.
    // This reduces lock contention and provides linear scalability.
    timers: Arc<Mutex<Timers>>,
}

impl SynchronizedTimers {
    pub(super) fn with_timers<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Timers) -> R,
    {
        let mut timers = self.timers.lock().expect("timers lock poisoned");
        f(&mut timers)
    }

    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub fn try_advance_timers(&self, now: Instant) -> Option<Instant> {
        self.with_timers(|timers| timers.advance_timers(now))
    }
}

#[derive(Debug, Clone, Default)]
pub enum GlobalState {
    #[default]
    System,
    #[cfg(any(feature = "test-util", test))]
    ClockControl(crate::ClockControl),
}

impl From<GlobalState> for ClockState {
    fn from(state: GlobalState) -> Self {
        match state {
            #[cfg(any(feature = "test-util", test))]
            GlobalState::ClockControl(control) => Self::ClockControl(control),
            GlobalState::System => Self::System(SynchronizedTimers::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_state_send_and_sync() {
        static_assertions::assert_impl_all!(ClockState: Send, Sync);
    }
}
