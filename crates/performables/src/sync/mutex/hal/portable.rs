// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{PoisonError, TryLockError};
use std::time::Duration;

// Targets without the address-wait HAL retain std's native blocking and
// condition-variable backend rather than introducing another platform protocol.
#[derive(Debug)]
pub(in crate::sync) struct Mutex {
    inner: std::sync::Mutex<()>,
}

pub(in crate::sync) type Guard<'a> = std::sync::MutexGuard<'a, ()>;

impl Mutex {
    pub(in crate::sync) const fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(()),
        }
    }

    pub(in crate::sync) fn try_lock(&self) -> Option<Guard<'_>> {
        match self.inner.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    pub(in crate::sync) fn lock(&self) -> Guard<'_> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[expect(clippy::unused_self, reason = "matches the address-wait HAL; std's guard retains its mutex")]
    pub(in crate::sync) fn unlock(&self, _guard: Guard<'_>) {}

    pub(in crate::sync) fn is_poisoned(&self) -> bool {
        self.inner.is_poisoned()
    }

    pub(in crate::sync) fn clear_poison(&self) -> bool {
        let poisoned = self.inner.is_poisoned();
        self.inner.clear_poison();
        poisoned
    }

    pub(in crate::sync) fn prepare_poison(&self) -> bool {
        // std writes the poison state when its owning guard is released.
        !self.inner.is_poisoned()
    }
}

#[derive(Debug)]
pub(in crate::sync) struct Condvar {
    inner: std::sync::Condvar,
}

impl Condvar {
    pub(in crate::sync) const fn new() -> Self {
        Self {
            inner: std::sync::Condvar::new(),
        }
    }

    pub(in crate::sync) fn wait<'a>(&self, _mutex: &'a Mutex, guard: Guard<'a>, timeout: Option<Duration>) -> (Guard<'a>, bool) {
        if let Some(timeout) = timeout {
            let (guard, outcome) = self.inner.wait_timeout(guard, timeout).unwrap_or_else(PoisonError::into_inner);
            (guard, outcome.timed_out())
        } else {
            (self.inner.wait(guard).unwrap_or_else(PoisonError::into_inner), false)
        }
    }

    pub(in crate::sync) fn notify_one(&self) {
        self.inner.notify_one();
    }

    pub(in crate::sync) fn notify_all(&self) {
        self.inner.notify_all();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use super::*;

    #[test]
    fn fallback_preserves_exclusion_and_poison_recovery() {
        let mutex = Mutex::new();
        let initially_poisoned = mutex.is_poisoned();
        let guard = mutex.try_lock().unwrap();
        let blocked = mutex.try_lock().is_none();
        mutex.unlock(guard);
        let prepared = Cell::new(false);
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = mutex.lock();
            prepared.set(mutex.prepare_poison());
            panic!("poison fallback");
        }))
        .unwrap_err();
        let recovered = mutex.try_lock().unwrap();
        let poisoned = mutex.is_poisoned();
        let repeated = mutex.prepare_poison();
        mutex.unlock(recovered);
        let recovered = mutex.lock();
        let cleared = mutex.clear_poison();
        mutex.unlock(recovered);
        assert_eq!(
            (
                blocked,
                prepared.get(),
                poisoned,
                repeated,
                cleared,
                mutex.clear_poison(),
                initially_poisoned,
                mutex.is_poisoned()
            ),
            (true, true, true, false, true, false, false, false)
        );
    }

    #[test]
    fn fallback_timeout_reacquires_a_poisoned_mutex() {
        let mutex = Mutex::new();
        let condition = Condvar::new();
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = mutex.lock();
            panic!("poison before timed wait");
        }))
        .unwrap_err();
        let (guard, timed_out) = condition.wait(&mutex, mutex.lock(), Some(Duration::ZERO));
        let blocked = mutex.try_lock().is_none();
        let poisoned = mutex.is_poisoned();
        mutex.unlock(guard);
        assert_eq!((timed_out, blocked, poisoned), (true, true, true));
    }

    #[test]
    fn fallback_notifications_reacquire_a_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new());
        let condition = Arc::new(Condvar::new());
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn({
            let mutex = Arc::clone(&mutex);
            let condition = Arc::clone(&condition);
            move || {
                let mut guard = mutex.lock();
                started_tx.send(()).unwrap();
                let result = loop {
                    let (next_guard, timed_out) = condition.wait(&mutex, guard, None);
                    guard = next_guard;
                    let poisoned = mutex.is_poisoned();
                    if poisoned {
                        break (timed_out, poisoned);
                    }
                };
                mutex.unlock(guard);
                finished_tx.send(result).unwrap();
            }
        });
        started_rx.recv().unwrap();
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = mutex.lock();
            panic!("poison after the waiter releases ownership");
        }))
        .unwrap_err();
        condition.notify_one();
        let deadline = if cfg!(miri) {
            Duration::from_mins(2)
        } else {
            Duration::from_secs(5)
        };
        let result = if testing_aids::is_mutation_testing() {
            finished_rx.recv().unwrap()
        } else {
            finished_rx.recv_timeout(deadline).unwrap()
        };
        worker.join().unwrap();
        assert_eq!(result, (false, true));
    }

    #[test]
    fn fallback_notify_all_releases_every_waiter() {
        const WAITERS: usize = 3;
        let mutex = Arc::new(Mutex::new());
        let condition = Arc::new(Condvar::new());
        let ready = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let workers = (0..WAITERS)
            .map(|_| {
                std::thread::spawn({
                    let mutex = Arc::clone(&mutex);
                    let condition = Arc::clone(&condition);
                    let ready = Arc::clone(&ready);
                    let started_tx = started_tx.clone();
                    let finished_tx = finished_tx.clone();
                    move || {
                        let mut guard = mutex.lock();
                        started_tx.send(()).unwrap();
                        while !ready.load(Ordering::Relaxed) {
                            guard = condition.wait(&mutex, guard, None).0;
                        }
                        mutex.unlock(guard);
                        finished_tx.send(()).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..WAITERS {
            started_rx.recv().unwrap();
        }
        let guard = mutex.lock();
        ready.store(true, Ordering::Relaxed);
        condition.notify_all();
        mutex.unlock(guard);
        let deadline = if cfg!(miri) {
            Duration::from_mins(2)
        } else {
            Duration::from_secs(5)
        };
        for _ in 0..WAITERS {
            if testing_aids::is_mutation_testing() {
                finished_rx.recv().unwrap();
            } else {
                finished_rx.recv_timeout(deadline).unwrap();
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }
}
