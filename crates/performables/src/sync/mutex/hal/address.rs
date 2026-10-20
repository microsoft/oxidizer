// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::Unlock;
#[cfg(target_os = "linux")]
use super::linux as platform;
#[cfg(target_os = "windows")]
use super::windows as platform;

const UNLOCKED: platform::LockWord = 0;
const LOCKED: platform::LockWord = 1;
const CONTENDED: platform::LockWord = 2;

// std::Mutex<()> plus a second owning guard leaves measured guard copies and
// cleanup on the uncontended path. One raw ownership state avoids those layers;
// address waits retain blocking behavior without allocation or busy waiting.
#[derive(Debug)]
pub(in crate::sync) struct Mutex {
    state: platform::AtomicLockWord,
    poisoned: AtomicBool,
}

#[derive(Debug)]
pub(in crate::sync) struct Guard<'a> {
    // Preserve native non-Send affinity without storing another owning guard.
    marker: PhantomData<std::sync::MutexGuard<'a, ()>>,
}

impl Mutex {
    pub(in crate::sync) const fn new() -> Self {
        Self {
            state: platform::AtomicLockWord::new(UNLOCKED),
            poisoned: AtomicBool::new(false),
        }
    }

    #[inline]
    pub(in crate::sync) fn try_lock(&self) -> Option<Guard<'_>> {
        self.state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Guard { marker: PhantomData })
    }

    pub(in crate::sync) fn lock(&self) -> Guard<'_> {
        if let Some(guard) = self.try_lock() {
            return guard;
        }
        self.lock_contended()
    }

    #[cold]
    fn lock_contended(&self) -> Guard<'_> {
        // A waiter acquires with CONTENDED retained so its release wakes the
        // next parked owner, including when other threads take the fast path.
        while self.state.swap(CONTENDED, Ordering::Acquire) != UNLOCKED {
            platform::wait_lock(&self.state, CONTENDED);
        }
        Guard { marker: PhantomData }
    }

    #[inline]
    pub(in crate::sync) fn unlock(&self, _guard: Guard<'_>) {
        if self.state.swap(UNLOCKED, Ordering::Release) == CONTENDED {
            platform::wake_lock(&self.state);
        }
    }

    #[inline]
    pub(in crate::sync) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    pub(in crate::sync) fn clear_poison(&self) -> bool {
        self.poisoned.swap(false, Ordering::Relaxed)
    }

    pub(in crate::sync) fn prepare_poison(&self) -> bool {
        // The subsequent mutex Release publishes this transition to owners.
        !self.poisoned.swap(true, Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub(in crate::sync) struct Condvar {
    generation: AtomicU32,
    // One extra word avoids kernel wakes once the last wait has returned.
    waiters: AtomicU32,
}

struct ConditionRegistration<'a> {
    waiters: &'a AtomicU32,
}

impl Drop for ConditionRegistration<'_> {
    fn drop(&mut self) {
        let previous = self.waiters.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0);
    }
}

impl Condvar {
    pub(in crate::sync) const fn new() -> Self {
        Self {
            generation: AtomicU32::new(0),
            waiters: AtomicU32::new(0),
        }
    }

    pub(in crate::sync) fn wait<'a>(&self, mutex: &'a Mutex, guard: Guard<'a>, timeout: Option<Duration>) -> (Guard<'a>, bool) {
        let ownership = Unlock::new(mutex, guard);
        // Capture while owning the mutex. Atomic expected-value parking then
        // notices notification between this release and the actual OS wait.
        let generation = self.generation.load(Ordering::SeqCst);
        let registration = self.register_waiter();
        let deadline = timeout.map(|duration| (duration, Instant::now()));
        drop(ownership);
        let timed_out = wait_with_timeout(deadline, |remaining| {
            platform::wait_condition(&self.generation, generation, remaining)
        });
        drop(registration);
        (mutex.lock(), timed_out)
    }

    fn register_waiter(&self) -> ConditionRegistration<'_> {
        // Registration precedes mutex release. Sequential consistency also
        // orders notifications that do not hold the associated mutex.
        self.waiters
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |waiters| waiters.checked_add(1))
            .expect("simultaneous condition-variable waits must not exceed u32::MAX");
        ConditionRegistration { waiters: &self.waiters }
    }

    #[inline]
    pub(in crate::sync) fn notify_one(&self) {
        self.notify_with(platform::wake_condition_one);
    }

    #[inline]
    pub(in crate::sync) fn notify_all(&self) {
        self.notify_with(platform::wake_condition_all);
    }

    #[inline]
    fn notify_with(&self, wake: impl FnOnce(&AtomicU32)) {
        if self.waiters.load(Ordering::SeqCst) != 0 {
            self.generation.fetch_add(1, Ordering::SeqCst);
            wake(&self.generation);
        }
    }
}

fn wait_with_timeout(deadline: Option<(Duration, Instant)>, mut wait: impl FnMut(Option<Duration>) -> bool) -> bool {
    let mut remaining = deadline.map(|(duration, _)| duration);
    loop {
        if !wait(remaining) {
            break false;
        }
        let Some((timeout, started)) = deadline else {
            unreachable!("an indefinite condition wait cannot time out");
        };
        let Some(next) = timeout.checked_sub(started.elapsed()) else {
            break true;
        };
        // OS duration ranges can be narrower than Duration. A saturated
        // wait is only one chunk, not an early expiry of the user's timeout.
        remaining = Some(next);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn fresh_raw_primitives_provide_reusable_exclusive_ownership() {
        let mutex = Mutex::new();
        let condition = Condvar::new();
        let first = mutex.try_lock().unwrap();
        let blocked = mutex.try_lock().is_none();
        let (next, timed_out) = condition.wait(&mutex, first, Some(Duration::ZERO));
        let held = mutex.try_lock().is_none();
        mutex.unlock(next);
        let reused = mutex.try_lock().unwrap();
        mutex.unlock(reused);
        assert_eq!((blocked, held, timed_out, mutex.is_poisoned()), (true, true, true, false));
    }

    #[test]
    fn condition_notifications_skip_empty_and_expired_registrations() {
        let condition = Condvar::new();
        condition.notify_with(|_| panic!("an empty condition must not request an OS wake"));
        let registration = condition.register_waiter();
        let mut wakes = 0;
        condition.notify_with(|_| wakes += 1);
        assert_eq!(condition.generation.load(Ordering::SeqCst), 1);
        drop(registration);
        condition.notify_with(|_| panic!("the final registration has already expired"));
        assert_eq!((wakes, condition.waiters.load(Ordering::SeqCst)), (1, 0));
    }

    #[test]
    fn condition_notifications_preserve_other_active_registrations() {
        let condition = Condvar::new();
        condition.notify_one();
        condition.notify_all();
        assert_eq!(condition.generation.load(Ordering::SeqCst), 0);
        let first = condition.register_waiter();
        let second = condition.register_waiter();
        condition.notify_one();
        drop(first);
        assert_eq!(condition.waiters.load(Ordering::SeqCst), 1);
        condition.notify_all();
        drop(second);
        assert_eq!(
            (
                condition.generation.load(Ordering::SeqCst),
                condition.waiters.load(Ordering::SeqCst),
            ),
            (2, 0),
        );
    }

    #[test]
    fn condition_registration_is_released_after_timeout_and_unwind() {
        let mutex = Mutex::new();
        let condition = Condvar::new();
        let (guard, timed_out) = condition.wait(&mutex, mutex.lock(), Some(Duration::ZERO));
        mutex.unlock(guard);
        let panic = std::panic::catch_unwind(|| {
            let _registration = condition.register_waiter();
            panic!("unwind while registered");
        })
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>().copied(), Some("unwind while registered"));
        assert_eq!((timed_out, condition.waiters.load(Ordering::SeqCst)), (true, 0));
    }

    #[test]
    fn condition_registration_overflow_releases_mutex_ownership() {
        let mutex = Mutex::new();
        let condition = Condvar::new();
        condition.waiters.store(u32::MAX, Ordering::SeqCst);
        let panic = std::panic::catch_unwind(|| {
            condition.wait(&mutex, mutex.lock(), Some(Duration::ZERO));
        })
        .unwrap_err();
        assert!(
            panic
                .downcast_ref::<String>()
                .is_some_and(|message| message.contains("simultaneous condition-variable waits")),
        );
        let guard = mutex.try_lock().unwrap();
        mutex.unlock(guard);
    }

    #[test]
    fn a_saturated_os_timeout_retries_the_remaining_user_duration() {
        let duration = Duration::from_secs(10);
        let mut calls = 0;
        let timed_out = wait_with_timeout(Some((duration, Instant::now())), |remaining| {
            assert!(remaining.unwrap() <= duration);
            calls += 1;
            calls == 1
        });
        assert_eq!((timed_out, calls), (false, 2));
    }

    #[test]
    #[should_panic(expected = "indefinite condition wait cannot time out")]
    fn an_invalid_native_timeout_is_not_reported_as_success() {
        wait_with_timeout(None, |_| true);
    }
}
