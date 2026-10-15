// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Waker;
use std::time::Instant;

use performables::arc::{Arc, PerThread};
use performables::sync::mutex::Mutex;
use thread_aware::{Thread, ThreadAware};

use crate::timers::Timers;

#[derive(Debug, Clone)]
pub(crate) enum ClockState {
    System(SystemClockState),
    #[cfg(any(feature = "test-util", test))]
    ClockControl(crate::ClockControl),
}

#[derive(Debug)]
pub(crate) struct SystemClockState {
    timers: SynchronizedTimers,
    clock_owner: bool,
}

impl SystemClockState {
    fn new(timers: SynchronizedTimers) -> Self {
        Self {
            timers,
            clock_owner: false,
        }
    }

    fn activate_clock_owner(&mut self) {
        debug_assert!(!self.clock_owner, "clock state can become an owner only once");
        self.timers.add_clock_owner();
        self.clock_owner = true;
    }
}

impl Clone for SystemClockState {
    fn clone(&self) -> Self {
        if self.clock_owner {
            self.timers.add_clock_owner();
        }
        Self {
            timers: self.timers.clone(),
            clock_owner: self.clock_owner,
        }
    }
}

impl Drop for SystemClockState {
    fn drop(&mut self) {
        if self.clock_owner {
            self.timers.remove_clock_owner();
        }
    }
}

impl Deref for SystemClockState {
    type Target = SynchronizedTimers;

    fn deref(&self) -> &Self::Target {
        &self.timers
    }
}

impl ThreadAware for SystemClockState {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        if !self.clock_owner {
            self.timers.relocate(source, destination);
            return;
        }

        let previous_timers = self.timers.clone();
        self.timers.relocate(source, destination);
        self.timers.add_clock_owner();
        previous_timers.remove_clock_owner();
    }
}

impl ThreadAware for ClockState {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        match self {
            Self::System(synchronized_timers) => synchronized_timers.relocate(source, destination),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(clock_control) => clock_control.relocate(source, destination),
        }
    }
}

impl ClockState {
    pub(crate) fn new_system() -> Self {
        Self::System(SystemClockState::new(SynchronizedTimers::new_isolated()))
    }

    /// Creates a `System` clock state backed by a single shared timer set. Used by clocks driven
    /// by a single global driver task (e.g. the Tokio-driven clock created by
    /// [`Clock::new_tokio`][crate::Clock::new_tokio]). [`ThreadAware::relocate`] is a no-op for
    /// this variant.
    #[cfg(any(feature = "rt-shared", test))]
    pub(crate) fn new_system_shared() -> Self {
        Self::System(SystemClockState::new(SynchronizedTimers::new_shared()))
    }

    pub(crate) fn activate_clock_owner(&mut self) {
        match self {
            Self::System(state) => state.activate_clock_owner(),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => {}
        }
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

    pub(crate) fn set_driver_waker(&self, waker: Waker) {
        match self {
            Self::System(timers) => timers.set_driver_waker(waker),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => {}
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    pub(crate) fn driver_is_finished(&self) -> bool {
        match self {
            Self::System(timers) => timers.has_no_clock_owners(),
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
    /// A single shared timer set. [`ThreadAware::relocate`] is a no-op, so all clones observe
    /// the same timers regardless of thread coordinate. Used by clocks driven by a single global
    /// driver task (e.g. the Tokio-driven clock created by [`Clock::new_tokio`][crate::Clock::new_tokio]).
    #[cfg(any(feature = "rt-shared", test))]
    Shared(std::sync::Arc<TimerState>),

    /// Per-thread isolated timer storage. Within one runtime owner,
    /// [`ThreadAware::relocate`] creates a fresh timer set on the destination
    /// thread, enabling thread-isolated runtimes to operate on independent
    /// timers with no cross-thread lock contention. A cross-owner move retains
    /// the current set instead.
    Isolated(Arc<TimerState, PerThread>),
}

#[derive(Debug, Default)]
pub(crate) struct TimerState {
    timers: Mutex<Timers>,
    driver_waker: Mutex<Option<Waker>>,
    clock_owners: AtomicUsize,
}

impl ThreadAware for SynchronizedTimers {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        match self {
            #[cfg(any(feature = "rt-shared", test))]
            Self::Shared(_) => {}
            Self::Isolated(timers) => timers.relocate(source, destination),
        }
    }
}

impl SynchronizedTimers {
    pub(crate) fn new_isolated() -> Self {
        Self::Isolated(Arc::new_with(TimerState::default))
    }

    #[cfg(any(feature = "rt-shared", test))]
    pub(crate) fn new_shared() -> Self {
        Self::Shared(std::sync::Arc::new(TimerState::default()))
    }

    fn state(&self) -> &TimerState {
        match self {
            #[cfg(any(feature = "rt-shared", test))]
            Self::Shared(state) => state,
            Self::Isolated(state) => state,
        }
    }

    pub(super) fn with_timers<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Timers) -> R,
    {
        let mut timers = self.state().timers.lock();
        f(&mut timers)
    }

    pub(crate) fn set_driver_waker(&self, waker: Waker) {
        *self.state().driver_waker.lock() = Some(waker);
    }

    pub(crate) fn add_clock_owner(&self) {
        self.state().clock_owners.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn remove_clock_owner(&self) {
        let previous = self.state().clock_owners.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "clock owner count must not underflow");
        if previous == 1 {
            self.wake_driver();
        }
    }

    pub(crate) fn has_no_clock_owners(&self) -> bool {
        self.state().clock_owners.load(Ordering::Acquire) == 0
    }

    fn wake_driver(&self) {
        let driver_waker = self.state().driver_waker.lock().clone();
        if let Some(driver_waker) = driver_waker {
            driver_waker.wake();
        }
    }

    pub(crate) fn register_timer(&self, when: Instant, waker: Waker) -> crate::timers::TimerKey {
        let (key, became_earliest) = self.with_timers(|timers| {
            let previous_next = timers.next_timer();
            let key = timers.register(when, waker);
            (key, previous_next.is_none_or(|previous| when < previous))
        });
        if became_earliest {
            self.wake_driver();
        }
        key
    }

    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub(crate) fn try_advance_timers(&self, now: Instant) -> Option<Instant> {
        self.with_timers(|timers| timers.advance_timers(now))
    }

    #[cfg(test)]
    pub(crate) fn is_unique(&self) -> bool {
        match self {
            #[cfg(any(feature = "rt-shared", test))]
            Self::Shared(timers) => std::sync::Arc::strong_count(timers) == 1,
            Self::Isolated(timers) => Arc::strong_count(timers) == 1,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_clock_state_is_send_and_sync() {
        // `ClockState` is a crate-internal representation shared by public clock types.
        static_assertions::assert_impl_all!(ClockState: Send, Sync);
    }

    #[test]
    fn private_shared_timer_uniqueness_tracks_internal_clones() {
        // Arc uniqueness is an implementation detail used by driver lifecycle detection.
        let timers = SynchronizedTimers::new_shared();
        assert!(timers.is_unique());

        let clone = timers.clone();
        assert!(!timers.is_unique());

        drop(clone);
        assert!(timers.is_unique());
    }
}
