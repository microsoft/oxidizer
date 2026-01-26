// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::timers::Timers;

#[derive(Debug, Clone)]
pub(crate) enum ClockState {
    System(SynchronizedTimers),
    #[cfg(any(feature = "test-util", test))]
    ClockControl(crate::ClockControl),
}

impl ClockState {
    pub fn new_system() -> Self {
        Self::System(SynchronizedTimers::default())
    }
}

impl ClockState {
    #[cfg(test)]
    pub(super) fn timers_len(&self) -> usize {
        match self {
            Self::ClockControl(control) => control.timers_len(),
            Self::System(timers) => timers.with_timers(|t| t.len()),
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    pub fn is_unique(&self) -> bool {
        match self {
            Self::System(timers) => timers.is_unique(),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(control) => control.is_unique(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SynchronizedTimers {
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
    pub(crate) fn try_advance_timers(&self, now: Instant) -> Option<Instant> {
        self.with_timers(|timers| timers.advance_timers(now))
    }

    pub fn is_unique(&self) -> bool {
        Arc::strong_count(&self.timers) == 1
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_state_send_and_sync() {
        static_assertions::assert_impl_all!(ClockState: Send, Sync);
    }
}
