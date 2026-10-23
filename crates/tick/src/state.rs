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

    /// Creates system-clock state with one shared timer set.
    ///
    /// A single driver task uses this state. For example, [`Clock::new_tokio`]
    /// uses it. [`ThreadAware::relocate`] does nothing for this state.
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

    pub(crate) fn register_driver(&self, waker: Waker) -> Option<DriverId> {
        match self {
            Self::System(storage) => Some(storage.register_driver(waker)),
            #[cfg(any(feature = "test-util", test))]
            Self::ClockControl(_) => None,
        }
    }

    #[expect(
        clippy::panic,
        reason = "a controlled clock never registers a driver ID; receiving one is an internal state invariant violation"
    )]
    pub(crate) fn unregister_driver(&self, driver_id: Option<DriverId>) {
        match (self, driver_id) {
            (Self::System(storage), Some(driver_id)) => storage.unregister_driver(driver_id),
            #[cfg(any(feature = "test-util", test))]
            (Self::ClockControl(_), Some(_)) => {
                panic!("controlled clocks do not register driver wakers");
            }
            (_, None) => {}
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

// The mutex here is not accessed on a hot path. Timers are accessed only when:
//
// 1. A new timer is registered.
// 2. A timer is unregistered.
// 3. Timers are evaluated. Timer evaluation is very fast when there are no timers to fire. If
//    there are timers to fire, the time to evaluate them is proportional to the number of timers
//    that are ready to fire, and taking the lock is not the bottleneck.
#[derive(Debug, Clone)]
pub(crate) enum SystemClockStorage {
    /// System-clock state with one shared timer set.
    ///
    /// [`ThreadAware::relocate`] does nothing. All clones use the same timers,
    /// driver registrations, and clock-owner count on all thread coordinates.
    /// A single driver task uses this state, such as [`Clock::new_tokio`].
    #[cfg(any(feature = "rt-shared", test))]
    Shared(std::sync::Arc<SystemClockStorageInner>),

    /// System-clock state with one timer set per thread.
    ///
    /// Within one runtime owner, [`ThreadAware::relocate`] creates storage on
    /// the destination thread. Each thread can then work without timer-lock
    /// contention from other threads. A move to another owner keeps the current
    /// storage.
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
        // A Vec would allocate on the timer hot path. A bounded stack batch handles ordinary timer
        // bursts with one lock acquisition and loops for larger bursts without imposing a limit.
        const READY_BATCH_SIZE: usize = 8;

        let mut first = [None];
        let (count, next) = self.with_timers(|timers| timers.pop_ready_into(now, &mut first));
        if count == 0 {
            return next;
        }

        first[0]
            .take()
            .expect("filled by pop_ready_into when one ready timer is reported")
            .wake();
        if next.is_none_or(|next| next > now) {
            return next;
        }

        let mut ready: [Option<Waker>; READY_BATCH_SIZE] = std::array::from_fn(|_| None);

        loop {
            let (count, next) = self.with_timers(|timers| timers.pop_ready_into(now, &mut ready));

            // A wake may synchronously re-enter timer code, so invoke each after releasing the lock.
            for waker in &mut ready[..count] {
                waker.take().expect("filled by pop_ready_into for every counted slot").wake();
            }

            if count < READY_BATCH_SIZE || next.is_none_or(|next| next > now) {
                return next;
            }
        }
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

    struct ReentrantWake {
        storage: SystemClockStorage,
        lock_was_available: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl std::task::Wake for ReentrantWake {
        fn wake(self: std::sync::Arc<Self>) {
            self.lock_was_available
                .store(self.storage.storage().timers.try_lock().is_some(), Ordering::Relaxed);
        }
    }

    struct ReentrantDropWake {
        storage: SystemClockStorage,
        lock_was_available: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl std::task::Wake for ReentrantDropWake {
        fn wake(self: std::sync::Arc<Self>) {
            self.lock_was_available
                .store(self.storage.storage().timers.try_lock().is_some(), Ordering::Relaxed);
        }
    }

    impl Drop for ReentrantDropWake {
        fn drop(&mut self) {
            self.lock_was_available
                .store(self.storage.storage().timers.try_lock().is_some(), Ordering::Relaxed);
        }
    }

    #[test]
    fn private_clock_state_is_send_and_sync() {
        // `ClockState` is a crate-internal representation shared by public clock types.
        static_assertions::assert_impl_all!(ClockState: Send, Sync);
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
    fn timer_wakers_run_after_releasing_system_timer_lock() {
        let storage = SystemClockStorage::new_shared();
        let lock_was_available = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waker = Waker::from(std::sync::Arc::new(ReentrantWake {
            storage: storage.clone(),
            lock_was_available: std::sync::Arc::clone(&lock_was_available),
        }));
        let now = Instant::now();
        _ = storage.register_timer(now, waker);

        storage.try_advance_timers(now);

        assert!(lock_was_available.load(Ordering::Relaxed));
    }

    #[test]
    fn displaced_wakers_are_dropped_after_releasing_system_timer_lock() {
        let storage = SystemClockStorage::new_shared();
        let clock = crate::Clock::new(ClockState::System(SystemClockState::new(storage.clone())));
        let lock_was_available = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waker = Waker::from(std::sync::Arc::new(ReentrantDropWake {
            storage,
            lock_was_available: std::sync::Arc::clone(&lock_was_available),
        }));
        let key = clock.register_timer(Instant::now() + std::time::Duration::from_secs(1), waker);

        assert!(clock.update_timer_waker(key, Waker::noop()));
        assert!(lock_was_available.load(Ordering::Relaxed));
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
        ClockState::ClockControl(crate::ClockControl::new()).unregister_driver(Some(0));
    }
}
