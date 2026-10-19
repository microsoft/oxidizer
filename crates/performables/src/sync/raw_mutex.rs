// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};

use super::mutex::hal;

// Internal state needs the same exclusion as the public Sync mutex, without
// recursively recording mutex events for each channel or waiter operation.
// The shared HAL supplies native parking and its existing portable fallback.
pub(super) struct RawMutex<T: ?Sized> {
    lock: hal::Mutex,
    value: UnsafeCell<T>,
}

// SAFETY: moving the lock transfers ownership of its payload.
unsafe impl<T: ?Sized + Send> Send for RawMutex<T> {}
// SAFETY: every payload access is protected by exclusive HAL ownership.
unsafe impl<T: ?Sized + Send> Sync for RawMutex<T> {}

// As with std::Mutex, unwinding does not invalidate the exclusion mechanism.
// Internal channel state has always recovered the standard lock after poison.
impl<T: ?Sized> std::panic::RefUnwindSafe for RawMutex<T> {}
impl<T: ?Sized> std::panic::UnwindSafe for RawMutex<T> {}

impl<T> RawMutex<T> {
    pub(super) const fn new(value: T) -> Self {
        Self {
            lock: hal::Mutex::new(),
            value: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> RawMutex<T> {
    pub(super) fn lock(&self) -> RawMutexGuard<'_, T> {
        // Keep the uncontended CAS visible to downstream monomorphizations;
        // calling the out-of-line HAL lock added a call to every value borrow.
        let guard = if let Some(guard) = self.lock.try_lock() {
            guard
        } else {
            self.lock.lock()
        };
        RawMutexGuard {
            mutex: self,
            ownership: ManuallyDrop::new(guard),
            _affinity: PhantomData,
        }
    }

    #[cfg(test)]
    pub(super) fn try_lock(&self) -> Option<RawMutexGuard<'_, T>> {
        self.lock.try_lock().map(|guard| RawMutexGuard {
            mutex: self,
            ownership: ManuallyDrop::new(guard),
            _affinity: PhantomData,
        })
    }
}

impl<T: ?Sized> fmt::Debug for RawMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMutex").finish_non_exhaustive()
    }
}

pub(super) struct RawMutexGuard<'a, T: ?Sized> {
    mutex: &'a RawMutex<T>,
    // Unlike a condition wait, this guard never transfers ownership. Retaining
    // only the raw proof avoids a second mutex pointer and an Option tag.
    ownership: ManuallyDrop<hal::Guard<'a>>,
    _affinity: PhantomData<std::sync::MutexGuard<'a, T>>,
}

// SAFETY: a shared guard exposes only &T, while its HAL ownership continues to
// exclude writers. Sharing it does not transfer ownership of a non-Send T.
unsafe impl<T: ?Sized + Sync> Sync for RawMutexGuard<'_, T> {}

impl<T: ?Sized> Deref for RawMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: construction acquired exclusive ownership, retained until
        // this guard drops; a shared borrow of the guard cannot mutate T.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T: ?Sized> DerefMut for RawMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: HAL ownership excludes other guards and &mut self excludes
        // all other borrows through this guard.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RawMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized> Drop for RawMutexGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: constructors acquire this mutex's ownership once. The private
        // field is never otherwise moved, and Rust runs this destructor once.
        let guard = unsafe { ManuallyDrop::take(&mut self.ownership) };
        self.mutex.lock.unlock(guard);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    #[test]
    fn ownership_is_exclusive_and_released_during_unwind() {
        let mutex = RawMutex::new(Cell::new(7));
        let guard = mutex.lock();
        assert!(mutex.try_lock().is_none());
        guard.set(11);
        drop(guard);
        assert_eq!(mutex.try_lock().unwrap().get(), 11);
        catch_unwind(AssertUnwindSafe(|| {
            let mut guard = mutex.lock();
            *guard = Cell::new(13);
            panic!("test unwinding while internal state is held");
        }))
        .unwrap_err();
        assert_eq!(mutex.lock().get(), 13);
        assert!(format!("{mutex:?}").contains("RawMutex"));
        assert_eq!(format!("{:?}", mutex.lock()), "Cell { value: 13 }");
    }
}
