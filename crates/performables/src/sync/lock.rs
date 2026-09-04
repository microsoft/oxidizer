// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use super::wait_queue::{WaitQueue, Waiter, block_on};
use super::{PoisonError, panic_poisoned};
use crate::telemetry::{self, EventKind};

const WRITER: usize = 1 << (usize::BITS - 1);
const WAITERS: usize = WRITER >> 1;
const READERS: usize = WAITERS - 1;

/// An executor-independent asynchronous reader-writer lock.
///
/// Uncontended reads and writes use atomic operations and do not allocate.
/// The lock is poisoned when an exclusive write guard is dropped during an
/// unwind that began after the guard was acquired. Read guards never poison it.
pub struct RwLock<T: ?Sized> {
    state: AtomicUsize,
    poisoned: AtomicBool,
    waiters: WaitQueue,
    value: UnsafeCell<T>,
}

// SAFETY: ownership of `T` can move with the lock when `T: Send`.
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
// SAFETY: shared access requires `T: Sync`; exclusive access is serialized.
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Creates an unlocked reader-writer lock containing `value`.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicUsize::new(0),
            poisoned: AtomicBool::new(false),
            waiters: WaitQueue::new(),
            value: UnsafeCell::new(value),
        }
    }

    /// Consumes the lock and returns its value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    /// Returns mutable access without locking because the lock is exclusively borrowed.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Returns a future that acquires shared access.
    ///
    /// # Panics
    ///
    /// Polling the returned future panics after acquiring the lock if a writer
    /// poisoned it.
    pub fn read(&self) -> RwLockRead<'_, T> {
        RwLockRead {
            result: self.read_result(),
        }
    }

    /// Returns a future that acquires shared access and reports poisoning.
    pub fn read_result(&self) -> RwLockReadResult<'_, T> {
        RwLockReadResult {
            lock: self,
            waiter: None,
            contention_recorded: false,
        }
    }

    /// Returns a future that acquires exclusive access.
    ///
    /// # Panics
    ///
    /// Polling the returned future panics after acquiring the lock if a writer
    /// poisoned it.
    pub fn write(&self) -> RwLockWrite<'_, T> {
        RwLockWrite {
            result: self.write_result(),
        }
    }

    /// Returns a future that acquires exclusive access and reports poisoning.
    pub fn write_result(&self) -> RwLockWriteResult<'_, T> {
        RwLockWriteResult {
            lock: self,
            waiter: None,
            contention_recorded: false,
        }
    }

    /// Blocks the current thread until shared access is acquired.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to release a conflicting guard.
    ///
    /// # Panics
    ///
    /// Panics after acquiring the lock if a writer poisoned it.
    pub fn read_sync(&self) -> RwLockReadGuard<'_, T> {
        match self.read_sync_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Blocks until shared access is acquired and reports poisoning.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to release a conflicting guard.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired read guard if a writer
    /// poisoned the lock.
    pub fn read_sync_result(&self) -> Result<RwLockReadGuard<'_, T>, PoisonError<RwLockReadGuard<'_, T>>> {
        if self.try_acquire_read() {
            return self.acquired_read();
        }

        self.record(EventKind::RwLockReadContention);
        block_on(RwLockReadResult {
            lock: self,
            waiter: None,
            contention_recorded: true,
        })
    }

    /// Blocks the current thread until exclusive access is acquired.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to release a conflicting guard.
    ///
    /// # Panics
    ///
    /// Panics after acquiring the lock if a writer poisoned it.
    pub fn write_sync(&self) -> RwLockWriteGuard<'_, T> {
        match self.write_sync_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Blocks until exclusive access is acquired and reports poisoning.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to release a conflicting guard.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired write guard if a writer
    /// poisoned the lock.
    pub fn write_sync_result(&self) -> Result<RwLockWriteGuard<'_, T>, PoisonError<RwLockWriteGuard<'_, T>>> {
        if self.try_acquire_write() {
            return self.acquired_write();
        }

        self.record(EventKind::RwLockWriteContention);
        block_on(RwLockWriteResult {
            lock: self,
            waiter: None,
            contention_recorded: true,
        })
    }

    /// Attempts to acquire shared access without waiting.
    ///
    /// # Panics
    ///
    /// Panics after acquiring the lock if a writer poisoned it.
    #[must_use]
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        match self.try_read_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Attempts to acquire shared access without waiting and reports poisoning.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired read guard if the lock was
    /// successfully acquired after a writer poisoned it.
    pub fn try_read_result(&self) -> Result<Option<RwLockReadGuard<'_, T>>, PoisonError<RwLockReadGuard<'_, T>>> {
        if self.try_acquire_read() {
            self.acquired_read().map(Some)
        } else {
            self.record(EventKind::RwLockReadContention);
            Ok(None)
        }
    }

    /// Attempts to acquire exclusive access without waiting.
    ///
    /// # Panics
    ///
    /// Panics after acquiring the lock if a writer poisoned it.
    #[must_use]
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        match self.try_write_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Attempts to acquire exclusive access without waiting and reports poisoning.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired write guard if the lock was
    /// successfully acquired after a writer poisoned it.
    pub fn try_write_result(&self) -> Result<Option<RwLockWriteGuard<'_, T>>, PoisonError<RwLockWriteGuard<'_, T>>> {
        if self.try_acquire_write() {
            self.acquired_write().map(Some)
        } else {
            self.record(EventKind::RwLockWriteContention);
            Ok(None)
        }
    }

    /// Returns whether the reader-writer lock is poisoned.
    ///
    /// The value can change immediately after this method returns when another
    /// thread holds the lock for writing.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// Clears the lock's poison state after the protected value is repaired.
    pub fn clear_poison(&self) {
        // AcqRel pairs with poison observations and publishes the cleared state
        // to acquisitions that subsequently check it.
        if self.poisoned.swap(false, Ordering::AcqRel) {
            self.record(EventKind::LockPoisonCleared);
        }
    }

    fn try_acquire_read(&self) -> bool {
        // A speculative increment keeps the uncontended path to one atomic
        // operation; writer and overflow observations are immediately rolled back.
        let previous = self.state.fetch_add(1, Ordering::Acquire);
        if previous & WRITER == 0 && previous & READERS != READERS {
            true
        } else {
            self.state.fetch_sub(1, Ordering::Release);
            false
        }
    }

    fn try_acquire_write(&self) -> bool {
        match self.state.compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => true,
            Err(WAITERS) => self
                .state
                .compare_exchange(WAITERS, WAITERS | WRITER, Ordering::Acquire, Ordering::Relaxed)
                .is_ok(),
            Err(_) => false,
        }
    }

    fn acquired_read(&self) -> Result<RwLockReadGuard<'_, T>, PoisonError<RwLockReadGuard<'_, T>>> {
        self.record(EventKind::RwLockReadAccess);
        let guard = RwLockReadGuard {
            lock: self,
            marker: PhantomData,
        };
        if self.is_poisoned() {
            self.record(EventKind::LockPoisonObserved);
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    fn acquired_write(&self) -> Result<RwLockWriteGuard<'_, T>, PoisonError<RwLockWriteGuard<'_, T>>> {
        self.record(EventKind::RwLockWriteAccess);
        let guard = RwLockWriteGuard {
            lock: self,
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

    fn unlock_read(&self) {
        let previous = self.state.fetch_sub(1, Ordering::Release);
        if previous & READERS == 1 && previous & WAITERS != 0 {
            self.wake_waiters();
        }
        self.record(EventKind::RwLockReadRelease);
    }

    fn unlock_write(&self) {
        let previous = self.state.fetch_and(!WRITER, Ordering::Release);
        if previous & WAITERS != 0 {
            self.wake_waiters();
        }
        self.record(EventKind::RwLockWriteRelease);
    }

    fn wake_waiters(&self) {
        self.waiters.wake_all_marked(|| {
            self.state.fetch_and(!WAITERS, Ordering::Release);
        });
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("RwLock");
        match self.try_read_result() {
            Ok(Some(value)) => debug.field("value", &&*value).field("poisoned", &false),
            Ok(None) => debug.field("value", &"<write-locked>").field("poisoned", &self.is_poisoned()),
            Err(error) => debug.field("value", &&**error.get_ref()).field("poisoned", &true),
        };
        debug.finish()
    }
}

/// A future that acquires shared access to an [`RwLock`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct RwLockRead<'a, T: ?Sized> {
    result: RwLockReadResult<'a, T>,
}

impl<'a, T: ?Sized> Future for RwLockRead<'a, T> {
    type Output = RwLockReadGuard<'a, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.result).poll(cx) {
            Poll::Ready(Ok(guard)) => Poll::Ready(guard),
            Poll::Ready(Err(error)) => panic_poisoned(&error),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A future that acquires shared access to an [`RwLock`] and reports poisoning.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct RwLockReadResult<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
    waiter: Option<Arc<Waiter>>,
    contention_recorded: bool,
}

impl<'a, T: ?Sized> Future for RwLockReadResult<'a, T> {
    type Output = Result<RwLockReadGuard<'a, T>, PoisonError<RwLockReadGuard<'a, T>>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.lock.try_acquire_read() {
            if let Some(waiter) = self.waiter.take() {
                self.lock.waiters.cancel_marked(&waiter, || {
                    self.lock.state.fetch_and(!WAITERS, Ordering::Release);
                });
            }
            return Poll::Ready(self.lock.acquired_read());
        }

        if !self.contention_recorded {
            self.lock.record(EventKind::RwLockReadContention);
            self.contention_recorded = true;
        }
        let lock = self.lock;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if lock.waiters.enqueue_if_needed_marked(
            &waiter,
            || {
                lock.state.fetch_or(WAITERS, Ordering::Release);
            },
            || lock.try_acquire_read(),
            || {
                lock.state.fetch_and(!WAITERS, Ordering::Release);
            },
        ) {
            self.waiter.take();
            Poll::Ready(lock.acquired_read())
        } else {
            Poll::Pending
        }
    }
}

impl<T: ?Sized> Drop for RwLockReadResult<'_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            self.lock.waiters.cancel_marked(waiter, || {
                self.lock.state.fetch_and(!WAITERS, Ordering::Release);
            });
        }
    }
}

/// A future that acquires exclusive access to an [`RwLock`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct RwLockWrite<'a, T: ?Sized> {
    result: RwLockWriteResult<'a, T>,
}

impl<'a, T: ?Sized> Future for RwLockWrite<'a, T> {
    type Output = RwLockWriteGuard<'a, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.result).poll(cx) {
            Poll::Ready(Ok(guard)) => Poll::Ready(guard),
            Poll::Ready(Err(error)) => panic_poisoned(&error),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A future that acquires exclusive access to an [`RwLock`] and reports poisoning.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct RwLockWriteResult<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
    waiter: Option<Arc<Waiter>>,
    contention_recorded: bool,
}

impl<'a, T: ?Sized> Future for RwLockWriteResult<'a, T> {
    type Output = Result<RwLockWriteGuard<'a, T>, PoisonError<RwLockWriteGuard<'a, T>>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.lock.try_acquire_write() {
            if let Some(waiter) = self.waiter.take() {
                self.lock.waiters.cancel_marked(&waiter, || {
                    self.lock.state.fetch_and(!WAITERS, Ordering::Release);
                });
            }
            return Poll::Ready(self.lock.acquired_write());
        }

        if !self.contention_recorded {
            self.lock.record(EventKind::RwLockWriteContention);
            self.contention_recorded = true;
        }
        let lock = self.lock;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if lock.waiters.enqueue_if_needed_marked(
            &waiter,
            || {
                lock.state.fetch_or(WAITERS, Ordering::Release);
            },
            || lock.try_acquire_write(),
            || {
                lock.state.fetch_and(!WAITERS, Ordering::Release);
            },
        ) {
            self.waiter.take();
            Poll::Ready(lock.acquired_write())
        } else {
            Poll::Pending
        }
    }
}

impl<T: ?Sized> Drop for RwLockWriteResult<'_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            self.lock.waiters.cancel_marked(waiter, || {
                self.lock.state.fetch_and(!WAITERS, Ordering::Release);
            });
        }
    }
}

/// A shared guard returned by [`RwLock::read`] and [`RwLock::try_read`].
pub struct RwLockReadGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
    marker: PhantomData<&'a T>,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the read count prevents an exclusive writer from acquiring.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock_read();
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

/// An exclusive guard returned by [`RwLock::write`] and [`RwLock::try_write`].
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<T>,
    panicking_at_acquisition: bool,
    marker: PhantomData<&'a mut T>,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: holding the write guard proves exclusive lock ownership.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: holding the write guard proves exclusive lock ownership.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        if !self.panicking_at_acquisition && std::thread::panicking() {
            self.lock.poison();
        }
        self.lock.unlock_write();
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
