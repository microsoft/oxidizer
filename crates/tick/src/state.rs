// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Mutex;
use std::time::Instant;

use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};
use thread_aware::{PerCore, ThreadAware};

use crate::timers::Timers;

#[derive(Debug, Clone)]
pub(crate) enum ClockState {
    System(SynchronizedTimers),
    #[cfg(any(feature = "test-util", test))]
    ClockControl(crate::ClockControl),
}

impl ThreadAware for ClockState {
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        match self {
            Self::System(synchronized_timers) => Self::System(synchronized_timers.relocated(source, destination)),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(clock_control) => Self::ClockControl(clock_control.relocated(source, destination)),
        }
    }
}

impl ClockState {
    pub fn new_system() -> Self {
        Self::System(SynchronizedTimers::new_isolated())
    }

    /// Creates a `System` clock state backed by a globally shared (non-isolated) timer set.
    /// Used by Tokio-driven clocks where a single background task drives the timers, so
    /// [`ThreadAware::relocated`] must be a no-op.
    #[cfg(any(feature = "tokio", test))]
    pub fn new_system_global() -> Self {
        Self::System(SynchronizedTimers::new_global())
    }
}

impl ClockState {
    pub(crate) fn timers_len(&self) -> usize {
        match self {
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(control) => control.timers_len(),
            Self::System(timers) => timers.with_timers(|t| t.len()),
        }
    }

    pub(crate) fn alive(&self) -> bool {
        match self {
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => true,
            Self::System(timers) => timers.with_timers(|t| t.alive()),
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

// The mutex here is not accessed on a hot path. Timers are accessed only when:
//
// 1. A new timer is registered.
// 2. A timer is unregistered.
// 3. Timers are evaluated. Timer evaluation is very fast when there are no timers to fire. If
//    there are timers to fire, the time to evaluate them is proportional to the number of timers
//    that are ready to fire, and taking the lock is not the bottleneck.
#[derive(Debug, Clone)]
pub(crate) enum SynchronizedTimers {
    /// A single shared timer set. [`ThreadAware::relocated`] is a no-op, so all clones observe
    /// the same timers regardless of thread affinity. Used by clocks driven by a single global
    /// driver task (e.g. the Tokio-driven clock created by [`Clock::new_tokio`][crate::Clock::new_tokio]).
    #[cfg(any(feature = "tokio", test))]
    Global(std::sync::Arc<Mutex<Timers>>),

    /// Per-core isolated timer storage. [`ThreadAware::relocated`] creates a fresh timer set on
    /// the destination core, enabling thread-per-core runtimes to operate on independent timers
    /// with no cross-thread lock contention.
    Isolated(thread_aware::Arc<Mutex<Timers>, PerCore>),
}

impl ThreadAware for SynchronizedTimers {
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        match self {
            #[cfg(any(feature = "tokio", test))]
            Self::Global(_) => self,
            Self::Isolated(timers) => Self::Isolated(timers.relocated(source, destination)),
        }
    }
}

impl SynchronizedTimers {
    pub fn new_isolated() -> Self {
        Self::Isolated(thread_aware::Arc::new(|| Mutex::new(Timers::default())))
    }

    #[cfg(any(feature = "tokio", test))]
    pub fn new_global() -> Self {
        Self::Global(std::sync::Arc::new(Mutex::new(Timers::default())))
    }

    pub(super) fn with_timers<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Timers) -> R,
    {
        let mut timers = match self {
            #[cfg(any(feature = "tokio", test))]
            Self::Global(timers) => timers.lock().expect("timers lock poisoned"),
            Self::Isolated(timers) => timers.lock().expect("timers lock poisoned"),
        };
        f(&mut timers)
    }

    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub(crate) fn try_advance_timers(&self, now: Instant) -> Option<Instant> {
        self.with_timers(|timers| timers.advance_timers(now))
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    pub fn is_unique(&self) -> bool {
        match self {
            #[cfg(any(feature = "tokio", test))]
            Self::Global(timers) => std::sync::Arc::strong_count(timers) == 1,
            Self::Isolated(timers) => thread_aware::Arc::strong_count(timers) == 1,
        }
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
