// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::wait_queue::{WaitQueue, Waiter, block_on};
use super::{PoisonError, panic_poisoned};
use crate::telemetry::{self, EventKind};

/// An executor-independent asynchronous mutual-exclusion lock.
///
/// The uncontended path uses one atomic compare-exchange and does not allocate.
/// The lock is poisoned when an exclusive guard is dropped during an unwind
/// that began after the guard was acquired.
pub struct Mutex<T: ?Sized> {
    locked: AtomicBool,
    poisoned: AtomicBool,
    waiters: WaitQueue,
    value: UnsafeCell<T>,
}

// SAFETY: ownership of `T` can move with the mutex when `T: Send`.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
// SAFETY: access to `T` is serialized by the lock state.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Creates an unlocked mutex containing `value`.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            waiters: WaitQueue::new(),
            value: UnsafeCell::new(value),
        }
    }

    /// Creates an unlocked mutex in a const context.
    #[must_use]
    pub const fn const_new(value: T) -> Self {
        Self::new(value)
    }

    /// Consumes the mutex and returns its value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Returns a future that acquires exclusive access.
    ///
    /// # Panics
    ///
    /// Polling the returned future panics after acquiring the lock if another
    /// thread poisoned it.
    pub fn lock(&self) -> MutexLock<'_, T> {
        MutexLock {
            result: self.lock_result(),
        }
    }

    /// Returns a future that acquires exclusive access and reports poisoning.
    pub fn lock_result(&self) -> MutexLockResult<'_, T> {
        MutexLockResult {
            mutex: self,
            waiter: None,
            contention_recorded: false,
        }
    }

    /// Blocks the current thread until exclusive access is acquired.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to make progress on the task
    /// currently holding the mutex.
    ///
    /// # Panics
    ///
    /// Panics after acquiring the lock if another thread poisoned it.
    pub fn lock_sync(&self) -> MutexGuard<'_, T> {
        match self.lock_sync_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Blocks until exclusive access is acquired and reports poisoning.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to make progress on the task
    /// currently holding the mutex.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired guard if another thread
    /// poisoned the mutex.
    pub fn lock_sync_result(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        if self.try_acquire() {
            return self.acquired();
        }

        self.record(EventKind::MutexContention);
        block_on(MutexLockResult {
            mutex: self,
            waiter: None,
            contention_recorded: true,
        })
    }

    /// Attempts to acquire exclusive access without waiting.
    ///
    /// # Panics
    ///
    /// Panics after acquiring the lock if another thread poisoned it.
    #[must_use]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self.try_lock_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Attempts to acquire exclusive access without waiting and reports poisoning.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired guard if the mutex was
    /// successfully acquired after another thread poisoned it.
    pub fn try_lock_result(&self) -> Result<Option<MutexGuard<'_, T>>, PoisonError<MutexGuard<'_, T>>> {
        if self.try_acquire() {
            self.acquired().map(Some)
        } else {
            self.record(EventKind::MutexContention);
            Ok(None)
        }
    }

    /// Returns whether the mutex is poisoned.
    ///
    /// The value can change immediately after this method returns when another
    /// thread holds the lock.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// Clears the mutex's poison state after the protected value is repaired.
    pub fn clear_poison(&self) {
        // AcqRel pairs with poison observations and publishes the cleared state
        // to acquisitions that subsequently check it.
        if self.poisoned.swap(false, Ordering::AcqRel) {
            self.record(EventKind::LockPoisonCleared);
        }
    }

    fn try_acquire(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn acquired(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        self.record(EventKind::MutexAccess);
        let guard = MutexGuard {
            mutex: self,
            panicking_at_acquisition: std::thread::panicking(),
            marker: PhantomData,
        };
        if self.is_poisoned() {
            self.record(EventKind::LockPoisonObserved);
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    fn poison(&self) {
        // Release publishes the poison transition before any later Acquire
        // observation; the lock's release/acquire pair orders the protected data.
        if self
            .poisoned
            .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            self.record(EventKind::LockPoisoned);
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        self.waiters.wake_one();
        self.record(EventKind::MutexRelease);
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl<T> Mutex<T> {
    /// Returns mutable access without locking because the mutex is exclusively borrowed.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized + Serialize> Serialize for Mutex<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.lock_sync().serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Mutex<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::new)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Mutex");
        match self.try_lock_result() {
            Ok(Some(value)) => debug.field("value", &&*value).field("poisoned", &false),
            Ok(None) => debug.field("value", &"<locked>").field("poisoned", &self.is_poisoned()),
            Err(error) => debug.field("value", &&**error.get_ref()).field("poisoned", &true),
        };
        debug.finish()
    }
}

/// A future that acquires a [`Mutex`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct MutexLock<'a, T: ?Sized> {
    result: MutexLockResult<'a, T>,
}

impl<'a, T: ?Sized> Future for MutexLock<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.result).poll(cx) {
            Poll::Ready(Ok(guard)) => Poll::Ready(guard),
            Poll::Ready(Err(error)) => panic_poisoned(&error),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A future that acquires a [`Mutex`] and reports poisoning.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct MutexLockResult<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    waiter: Option<Arc<Waiter>>,
    contention_recorded: bool,
}

impl<'a, T: ?Sized> Future for MutexLockResult<'a, T> {
    type Output = Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.mutex.try_acquire() {
            if let Some(waiter) = self.waiter.take() {
                self.mutex.waiters.cancel(&waiter);
            }
            return Poll::Ready(self.mutex.acquired());
        }

        if !self.contention_recorded {
            self.mutex.record(EventKind::MutexContention);
            self.contention_recorded = true;
        }
        let mutex = self.mutex;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if mutex.waiters.enqueue_if_needed(&waiter, || mutex.try_acquire()) {
            self.waiter.take();
            Poll::Ready(mutex.acquired())
        } else {
            Poll::Pending
        }
    }
}

impl<T: ?Sized> Drop for MutexLockResult<'_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            let removed = self.mutex.waiters.cancel(waiter);
            if !removed && !self.mutex.locked.load(Ordering::Acquire) {
                self.mutex.waiters.wake_one();
            }
        }
    }
}

/// An exclusive guard returned by [`Mutex::lock`] and [`Mutex::try_lock`].
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    panicking_at_acquisition: bool,
    marker: PhantomData<&'a mut T>,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: holding the guard proves exclusive lock ownership.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    pub(super) const fn mutex(&self) -> &'a Mutex<T> {
        self.mutex
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: holding the guard proves exclusive lock ownership.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        if !self.panicking_at_acquisition && std::thread::panicking() {
            self.mutex.poison();
        }
        self.mutex.unlock();
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
