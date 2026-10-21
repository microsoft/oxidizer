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

pub(crate) type DriverId = usize;

/// Carries a system-clock waker registration through driver ownership; controlled clocks carry no registration.
#[derive(Clone, Copy)]
pub(crate) struct DriverRegistration(Option<DriverId>);

impl DriverRegistration {
    fn registered(driver_id: DriverId) -> Self {
        Self(Some(driver_id))
    }

    #[cfg(any(feature = "test-util", test))]
    const fn not_required() -> Self {
        Self(None)
    }

    const fn driver_id(self) -> Option<DriverId> {
        self.0
    }
}

impl std::fmt::Debug for DriverRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug)]
pub(crate) struct SystemClockState {
    storage: SystemClockStorage,
    clock_owner: bool,
}

impl SystemClockState {
    fn new(storage: SystemClockStorage) -> Self {
        Self {
            storage,
            clock_owner: false,
        }
    }

    fn activate_clock_owner(&mut self) {
        debug_assert!(!self.clock_owner, "clock state can become an owner only once");
        self.storage.add_clock_owner();
        self.clock_owner = true;
    }
}

impl Clone for SystemClockState {
    fn clone(&self) -> Self {
        if self.clock_owner {
            self.storage.add_clock_owner();
        }
        Self {
            storage: self.storage.clone(),
            clock_owner: self.clock_owner,
        }
    }
}

impl Drop for SystemClockState {
    fn drop(&mut self) {
        if self.clock_owner {
            self.storage.remove_clock_owner();
        }
    }
}

impl Deref for SystemClockState {
    type Target = SystemClockStorage;

    fn deref(&self) -> &Self::Target {
        &self.storage
    }
}

impl ThreadAware for SystemClockState {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        if !self.clock_owner {
            self.storage.relocate(source, destination);
            return;
        }

        let previous_storage = self.storage.clone();
        self.storage.relocate(source, destination);
        self.storage.add_clock_owner();
        previous_storage.remove_clock_owner();
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
        Self::System(SystemClockState::new(SystemClockStorage::new_isolated()))
    }

    /// Creates a `System` clock state backed by a single shared timer set. Used by clocks driven
    /// by a single global driver task (e.g. the Tokio-driven clock created by
    /// [`Clock::new_tokio`][crate::Clock::new_tokio]). [`ThreadAware::relocate`] is a no-op for
    /// this variant.
    #[cfg(any(feature = "rt-shared", test))]
    pub(crate) fn new_system_shared() -> Self {
        Self::System(SystemClockState::new(SystemClockStorage::new_shared()))
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
            Self::System(storage) => storage.with_timers(|timers| timers.len()),
        }
    }

    pub(crate) fn alive(&self) -> bool {
        match self {
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => true,
            Self::System(storage) => storage.with_timers(|timers| timers.alive()),
        }
    }

    pub(crate) fn register_driver(&self, waker: Waker) -> DriverRegistration {
        match self {
            Self::System(storage) => DriverRegistration::registered(storage.register_driver(waker)),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => DriverRegistration::not_required(),
        }
    }

    pub(crate) fn unregister_driver(&self, registration: DriverRegistration) {
        match self {
            Self::System(storage) => {
                if let Some(driver_id) = registration.driver_id() {
                    storage.unregister_driver(driver_id);
                }
            }
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => unregister_controlled_driver(registration),
        }
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeout
    pub(crate) fn driver_is_finished(&self) -> bool {
        match self {
            Self::System(storage) => storage.has_no_clock_owners(),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(control) => control.is_unique(),
        }
    }
}

#[cfg(any(feature = "test-util", test))]
fn unregister_controlled_driver(registration: DriverRegistration) {
    assert!(
        registration.driver_id().is_none(),
        "controlled clocks do not register driver wakers"
    );
}

// The mutex here is not accessed on a hot path. Timers are accessed only when:
//
// 1. A new timer is registered.
// 2. A timer is unregistered.
// 3. Timers are evaluated. Timer evaluation is very fast when there are no timers to fire. If
//    there are timers to fire, the time to evaluate them is proportional to the number of timers
//    that are ready to fire, and taking the lock is not the bottleneck.
#[derive(Debug, Clone)]
pub(crate) enum SystemClockStorage {
    /// A single shared system-clock state. [`ThreadAware::relocate`] is a no-op, so all clones
    /// observe the same timers, driver registrations, and clock-owner count regardless of thread
    /// coordinate. Used by clocks driven by a single global driver task, such as
    /// [`Clock::new_tokio`][crate::Clock::new_tokio].
    #[cfg(any(feature = "rt-shared", test))]
    Shared(std::sync::Arc<SystemClockStorageInner>),

    /// Per-thread isolated system-clock state. Within one runtime owner,
    /// [`ThreadAware::relocate`] creates fresh storage on the destination thread, enabling
    /// thread-isolated runtimes to operate independently with no cross-thread lock contention.
    /// A cross-owner move retains the current storage instead.
    Isolated(Arc<SystemClockStorageInner, PerThread>),
}

#[derive(Debug, Default)]
pub(crate) struct SystemClockStorageInner {
    timers: Mutex<Timers>,
    driver_wakers: Mutex<Vec<(DriverId, std::sync::Arc<Waker>)>>,
    next_driver_id: AtomicUsize,
    clock_owners: AtomicUsize,
}

impl ThreadAware for SystemClockStorage {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        match self {
            #[cfg(any(feature = "rt-shared", test))]
            Self::Shared(_) => {}
            Self::Isolated(timers) => timers.relocate(source, destination),
        }
    }
}

impl SystemClockStorage {
    pub(crate) fn new_isolated() -> Self {
        Self::Isolated(Arc::new_with(SystemClockStorageInner::default))
    }

    #[cfg(any(feature = "rt-shared", test))]
    pub(crate) fn new_shared() -> Self {
        Self::Shared(std::sync::Arc::new(SystemClockStorageInner::default()))
    }

    fn storage(&self) -> &SystemClockStorageInner {
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
        let mut timers = self.storage().timers.lock();
        f(&mut timers)
    }

    pub(crate) fn register_driver(&self, waker: Waker) -> DriverId {
        let driver_id = self.storage().next_driver_id.fetch_add(1, Ordering::Relaxed);
        self.storage().driver_wakers.lock().push((driver_id, std::sync::Arc::new(waker)));
        driver_id
    }

    pub(crate) fn unregister_driver(&self, driver_id: DriverId) {
        let removed = {
            let mut driver_wakers = self.storage().driver_wakers.lock();
            driver_wakers
                .iter()
                .position(|(id, _)| *id == driver_id)
                .map(|index| driver_wakers.swap_remove(index).1)
        };
        drop(removed);
    }

    pub(crate) fn add_clock_owner(&self) {
        self.storage().clock_owners.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn remove_clock_owner(&self) {
        let previous = self.storage().clock_owners.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "clock owner count must not underflow");
        if previous == 1 {
            self.wake_drivers();
        }
    }

    pub(crate) fn has_no_clock_owners(&self) -> bool {
        self.storage().clock_owners.load(Ordering::Acquire) == 0
    }

    fn wake_drivers(&self) {
        let (first, remaining) = {
            let driver_wakers = self.storage().driver_wakers.lock();
            let mut wakers = driver_wakers.iter().map(|(_, waker)| std::sync::Arc::clone(waker));
            let Some(first) = wakers.next() else {
                return;
            };
            (first, wakers.collect::<Vec<_>>())
        };

        first.wake_by_ref();
        for waker in remaining {
            waker.wake_by_ref();
        }
    }

    pub(crate) fn register_timer(&self, when: Instant, waker: Waker) -> crate::timers::TimerKey {
        let (key, became_earliest) = self.with_timers(|timers| {
            let previous_next = timers.next_timer();
            let key = timers.register(when, waker);
            (key, previous_next.is_none_or(|previous| when < previous))
        });
        if became_earliest {
            self.wake_drivers();
        }
        key
    }

    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub(crate) fn try_advance_timers(&self, now: Instant) -> Option<Instant> {
        self.with_timers(|timers| timers.advance_timers(now))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct CountingWake(AtomicUsize);

    impl std::task::Wake for CountingWake {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn private_clock_state_is_send_and_sync() {
        // `ClockState` is a crate-internal representation shared by public clock types.
        static_assertions::assert_impl_all!(ClockState: Send, Sync);
    }

    #[test]
    fn private_driver_registration_preserves_option_debug_output() {
        let actual = (
            format!("{:?}", DriverRegistration::registered(7)),
            format!("{:?}", DriverRegistration::not_required()),
        );

        assert_eq!(actual, (String::from("Some(7)"), String::from("None")));
    }

    #[test]
    fn private_driver_notifications_follow_fixed_absolute_deadline_ordering() {
        // Exact absolute registration instants are private; public delays derive their deadline
        // from fresh clock reads and therefore cannot deterministically exercise every ordering.
        let storage = SystemClockStorage::new_shared();
        let wake_count = std::sync::Arc::new(CountingWake(AtomicUsize::new(0)));
        _ = storage.register_driver(Waker::from(std::sync::Arc::clone(&wake_count)));
        let now = Instant::now();

        _ = storage.register_timer(now + std::time::Duration::from_secs(2), Waker::noop().clone());
        assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);

        _ = storage.register_timer(now + std::time::Duration::from_secs(3), Waker::noop().clone());
        _ = storage.register_timer(now + std::time::Duration::from_secs(2), Waker::noop().clone());
        assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);

        _ = storage.register_timer(now + std::time::Duration::from_secs(1), Waker::noop().clone());
        assert_eq!(wake_count.0.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn private_clock_owner_activation_is_visible_to_driver_liveness() {
        let mut state = ClockState::new_system();
        assert!(state.driver_is_finished());

        state.activate_clock_owner();
        assert!(!state.driver_is_finished());
    }

    #[test]
    #[should_panic(expected = "controlled clocks do not register driver wakers")]
    fn private_controlled_clock_rejects_driver_unregistration() {
        ClockState::ClockControl(crate::ClockControl::new()).unregister_driver(DriverRegistration::registered(0));
    }
}
