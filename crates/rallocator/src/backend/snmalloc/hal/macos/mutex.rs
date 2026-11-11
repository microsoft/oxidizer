// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Inline allocator locking: Darwin's std mutex allocates on its first lock.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{PoisonError, TryLockError};

#[link(name = "System")]
unsafe extern "C" {
    fn os_unfair_lock_lock(lock: *mut u32);
    fn os_unfair_lock_trylock(lock: *mut u32) -> bool;
    fn os_unfair_lock_unlock(lock: *mut u32);
}

pub(crate) struct Mutex<T> {
    lock: UnsafeCell<u32>,
    poisoned: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: The native lock grants exactly one exclusive value lease. A value
// crossing between lock owners must be Send, as for std::sync::Mutex.
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self {
            lock: UnsafeCell::new(0), // OS_UNFAIR_LOCK_INIT.
            poisoned: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        // SAFETY: The initialized four-byte native lock remains stationary
        // while shared borrows or guards exist; the pool itself is static.
        unsafe { os_unfair_lock_lock(self.lock.get()) };
        self.guard()
    }

    pub(crate) fn try_lock(&self) -> Result<MutexGuard<'_, T>, TryLockError<MutexGuard<'_, T>>> {
        // SAFETY: The inline lock remains stationary during this shared borrow.
        // A successful attempt grants this thread the exclusive value lease.
        if unsafe { os_unfair_lock_trylock(self.lock.get()) } {
            self.guard().map_err(TryLockError::Poisoned)
        } else {
            Err(TryLockError::WouldBlock)
        }
    }

    #[cfg(test)]
    pub(crate) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    fn guard(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        let guard = MutexGuard {
            mutex: self,
            panicking: std::thread::panicking(),
            _not_send: PhantomData,
        };
        if self.poisoned.load(Ordering::Relaxed) {
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }
}

pub(crate) struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    panicking: bool,
    // The native lock must be unlocked by the same thread that acquired it.
    _not_send: PhantomData<*mut ()>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: This guard holds the native lock's unique value lease.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The guard owns the value lease and is exclusively borrowed.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        if !self.panicking && std::thread::panicking() {
            self.mutex.poisoned.store(true, Ordering::Relaxed);
        }
        // SAFETY: The non-Send guard releases its own live lock, on the same
        // thread that acquired it, after finishing all value accesses.
        unsafe { os_unfair_lock_unlock(self.mutex.lock.get()) };
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn concurrent_leases_serialize_mutation() {
        let value = Mutex::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..1000 {
                        *value.lock().unwrap() += 1;
                    }
                });
            }
        });
        assert_eq!(*value.lock().unwrap(), 8000);
    }

    #[test]
    fn first_try_lock_and_contended_retry_use_the_inline_lock() {
        let value = Mutex::new(0);
        let mut first = value.try_lock().unwrap();
        *first = 42;
        assert!(matches!(value.try_lock(), Err(TryLockError::WouldBlock)));
        drop(first);
        assert_eq!(*value.try_lock().unwrap(), 42);
        assert!(!value.is_poisoned());
    }

    #[test]
    fn unwinding_poisons_the_pool_lock_and_releases_its_lease() {
        let value = Mutex::new(0);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut guard = value.lock().unwrap();
            *guard = 42;
            panic!("poison the exclusively owned value");
        }));
        assert!(outcome.is_err());
        let Err(error) = value.lock() else {
            panic!("the unwinding owner must poison its mutex");
        };
        let guard = error.into_inner();
        assert_eq!(*guard, 42);
        drop(guard);
        assert!(value.is_poisoned());
        let Err(TryLockError::Poisoned(error)) = value.try_lock() else {
            panic!("try_lock must report poisoning after acquiring the released lock");
        };
        assert_eq!(*error.into_inner(), 42);
    }

    #[test]
    fn acquisition_during_unwinding_does_not_poison_the_lock() {
        struct LockOnDrop<'a>(&'a Mutex<usize>);

        impl Drop for LockOnDrop<'_> {
            fn drop(&mut self) {
                *self.0.lock().unwrap() += 1;
            }
        }

        let value = Mutex::new(0);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _on_drop = LockOnDrop(&value);
            panic!("exercise an already-unwinding lock acquisition");
        }));
        assert!(outcome.is_err());
        assert_eq!(*value.lock().unwrap(), 1);
    }
}
