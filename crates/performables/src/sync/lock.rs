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

use super::mode::{Async, Mode, Sync};

mod native;

use super::wait_queue::{WaitQueue, Waiter, block_on};
use super::{PoisonError, panic_poisoned};
use crate::telemetry::{self, EventKind};

const WRITER: usize = 1 << (usize::BITS - 1);
const WAITERS: usize = WRITER >> 1;
const READERS: usize = WAITERS - 1;

/// A reader-writer lock with a compile-time synchronization strategy.
///
/// The default [`Sync`] mode uses a compact native blocking lock. Select
/// [`Async`] to additionally use [`read_async`](RwLock::read_async) and
/// [`write_async`](RwLock::write_async).
///
/// Uncontended reads and writes use atomic operations and do not allocate.
/// The lock is poisoned when an exclusive write guard is dropped during an
/// unwind that began after the guard was acquired. Read guards never poison it.
pub struct RwLock<T: ?Sized, M: Mode = Sync> {
    // As for Mutex, a common UnsafeCell permits generic const construction
    // without per-mode constructor ambiguity. Each sealed strategy retains
    // native or atomic ownership evidence before accessing this cell.
    raw: M::RwLockState,
    value: UnsafeCell<T>,
}

/// Internal asynchronous reader-writer lock ownership state.
#[doc(hidden)]
#[derive(Debug)]
pub struct StateAsync {
    state: AtomicUsize,
    poisoned: AtomicBool,
    waiters: WaitQueue,
}

impl StateAsync {
    pub(super) const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            poisoned: AtomicBool::new(false),
            waiters: WaitQueue::new(),
        }
    }
}

// SAFETY: ownership of `T` can move with the lock when `T: Send`.
unsafe impl<T: ?Sized + Send, M: Mode> Send for RwLock<T, M> {}
// SAFETY: shared access requires `T: Sync`; exclusive access is serialized.
unsafe impl<T: ?Sized + Send + std::marker::Sync, M: Mode> std::marker::Sync for RwLock<T, M> {}

impl<T, M: Mode> RwLock<T, M> {
    /// Creates an unlocked reader-writer lock containing `value`.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            raw: M::RWLOCK_INIT,
            value: UnsafeCell::new(value),
        }
    }
}

#[expect(clippy::type_complexity, reason = "poison errors retain the acquired mode-specific guard")]
impl<T: ?Sized, M: Mode> RwLock<T, M> {
    /// Blocks until shared access is acquired.
    ///
    /// Do not block an executor thread needed to release a conflicting guard.
    ///
    /// # Panics
    ///
    /// Panics after acquisition if a writer poisoned the lock.
    pub fn read(&self) -> RwLockReadGuard<'_, T, M> {
        match self.read_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Blocks for shared access, returning a usable guard on poison.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] containing the acquired guard when poisoned.
    pub fn read_result(&self) -> Result<RwLockReadGuard<'_, T, M>, PoisonError<RwLockReadGuard<'_, T, M>>> {
        M::rwlock_read(self)
    }

    /// Attempts shared access without waiting.
    ///
    /// # Panics
    ///
    /// Panics after a successful acquisition if the lock is poisoned.
    #[must_use]
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T, M>> {
        match self.try_read_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Attempts shared access without waiting, returning a usable guard on poison.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] containing the acquired guard when poisoned.
    pub fn try_read_result(&self) -> Result<Option<RwLockReadGuard<'_, T, M>>, PoisonError<RwLockReadGuard<'_, T, M>>> {
        M::rwlock_try_read(self)
    }

    /// Blocks until exclusive access is acquired.
    ///
    /// Do not block an executor thread needed to release a conflicting guard.
    ///
    /// # Panics
    ///
    /// Panics after acquisition if a writer poisoned the lock.
    pub fn write(&self) -> RwLockWriteGuard<'_, T, M> {
        match self.write_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Blocks for exclusive access, returning a usable guard on poison.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] containing the acquired guard when poisoned.
    pub fn write_result(&self) -> Result<RwLockWriteGuard<'_, T, M>, PoisonError<RwLockWriteGuard<'_, T, M>>> {
        M::rwlock_write(self)
    }

    /// Attempts exclusive access without waiting.
    ///
    /// # Panics
    ///
    /// Panics after a successful acquisition if the lock is poisoned.
    #[must_use]
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T, M>> {
        match self.try_write_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Attempts exclusive access without waiting, returning a usable guard on poison.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] containing the acquired guard when poisoned.
    pub fn try_write_result(&self) -> Result<Option<RwLockWriteGuard<'_, T, M>>, PoisonError<RwLockWriteGuard<'_, T, M>>> {
        M::rwlock_try_write(self)
    }

    /// Returns whether a writer poisoned this lock.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        M::rwlock_is_poisoned(self)
    }

    /// Clears poisoning after the protected value has been repaired.
    pub fn clear_poison(&self) {
        M::rwlock_clear_poison(self);
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl<T, M: Mode> RwLock<T, M> {
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

#[expect(clippy::type_complexity, reason = "poison errors retain the acquired asynchronous guard")]
impl<T: ?Sized> RwLock<T, Async> {
    /// Returns a future that acquires shared access.
    ///
    /// # Panics
    ///
    /// Polling the returned future panics after acquiring the lock if a writer
    /// poisoned it.
    pub fn read_async(&self) -> RwLockRead<'_, T> {
        RwLockRead {
            result: self.read_async_result(),
        }
    }

    /// Returns a future that acquires shared access and reports poisoning.
    pub fn read_async_result(&self) -> RwLockReadResult<'_, T> {
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
    pub fn write_async(&self) -> RwLockWrite<'_, T> {
        RwLockWrite {
            result: self.write_async_result(),
        }
    }

    /// Returns a future that acquires exclusive access and reports poisoning.
    pub fn write_async_result(&self) -> RwLockWriteResult<'_, T> {
        RwLockWriteResult {
            lock: self,
            waiter: None,
            contention_recorded: false,
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
    pub(super) fn read_result_inner(&self) -> Result<RwLockReadGuard<'_, T, Async>, PoisonError<RwLockReadGuard<'_, T, Async>>> {
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

    /// Blocks until exclusive access is acquired and reports poisoning.
    ///
    /// The uncontended path does not allocate. This method must not be called
    /// from an executor thread that is required to release a conflicting guard.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired write guard if a writer
    /// poisoned the lock.
    pub(super) fn write_result_inner(&self) -> Result<RwLockWriteGuard<'_, T, Async>, PoisonError<RwLockWriteGuard<'_, T, Async>>> {
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

    /// Attempts to acquire shared access without waiting and reports poisoning.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired read guard if the lock was
    /// successfully acquired after a writer poisoned it.
    pub(super) fn try_read_result_inner(
        &self,
    ) -> Result<Option<RwLockReadGuard<'_, T, Async>>, PoisonError<RwLockReadGuard<'_, T, Async>>> {
        if self.try_acquire_read() {
            self.acquired_read().map(Some)
        } else {
            self.record(EventKind::RwLockReadContention);
            Ok(None)
        }
    }

    /// Attempts to acquire exclusive access without waiting and reports poisoning.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired write guard if the lock was
    /// successfully acquired after a writer poisoned it.
    pub(super) fn try_write_result_inner(
        &self,
    ) -> Result<Option<RwLockWriteGuard<'_, T, Async>>, PoisonError<RwLockWriteGuard<'_, T, Async>>> {
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
    pub(super) fn is_poisoned_inner(&self) -> bool {
        self.raw.poisoned.load(Ordering::Acquire)
    }

    /// Clears the lock's poison state after the protected value is repaired.
    pub(super) fn clear_poison_inner(&self) {
        // AcqRel pairs with poison observations and publishes the cleared state
        // to acquisitions that subsequently check it.
        if self.raw.poisoned.swap(false, Ordering::AcqRel) {
            self.record(EventKind::LockPoisonCleared);
        }
    }

    fn try_acquire_read(&self) -> bool {
        // A speculative increment can carry a saturated reader count into
        // WAITERS before rollback. Only enqueue may set that bit, after the
        // lazy wait-queue state is published, so reject overflow before writing.
        self.raw
            .state
            .fetch_update(Ordering::Acquire, Ordering::Relaxed, |state| {
                (state & WRITER == 0 && state & READERS != READERS).then(|| state + 1)
            })
            .is_ok()
    }

    fn try_acquire_write(&self) -> bool {
        match self.raw.state.compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => true,
            Err(WAITERS) => self
                .raw
                .state
                .compare_exchange(WAITERS, WAITERS + WRITER, Ordering::Acquire, Ordering::Relaxed)
                .is_ok(),
            Err(_) => false,
        }
    }

    fn acquired_read(&self) -> Result<RwLockReadGuard<'_, T, Async>, PoisonError<RwLockReadGuard<'_, T, Async>>> {
        self.record(EventKind::RwLockReadAccess);
        let guard = RwLockReadGuard {
            lock: self,
            raw: (),
            marker: PhantomData,
        };
        if self.is_poisoned() {
            self.record(EventKind::LockPoisonObserved);
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    fn acquired_write(&self) -> Result<RwLockWriteGuard<'_, T, Async>, PoisonError<RwLockWriteGuard<'_, T, Async>>> {
        self.record(EventKind::RwLockWriteAccess);
        let guard = RwLockWriteGuard {
            lock: self,
            panicking_at_acquisition: std::thread::panicking(),
            raw: (),
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
            .raw
            .poisoned
            .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            self.record(EventKind::LockPoisoned);
        }
    }

    fn unlock_read(&self) {
        let previous = self.raw.state.fetch_sub(1, Ordering::Release);
        if previous & READERS == 1 && previous & WAITERS == WAITERS {
            self.wake_waiters();
        }
        self.record(EventKind::RwLockReadRelease);
    }

    fn unlock_write(&self) {
        let previous = self.raw.state.fetch_and(!WRITER, Ordering::Release);
        if previous & WAITERS == WAITERS {
            self.wake_waiters();
        }
        self.record(EventKind::RwLockWriteRelease);
    }

    fn wake_waiters(&self) {
        self.raw.waiters.wake_all_marked(|| {
            self.raw.state.fetch_and(!WAITERS, Ordering::Release);
        });
    }
}

impl<T: Default, M: Mode> Default for RwLock<T, M> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized + fmt::Debug, M: Mode> fmt::Debug for RwLock<T, M> {
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
    type Output = RwLockReadGuard<'a, T, Async>;

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
    lock: &'a RwLock<T, Async>,
    waiter: Option<Arc<Waiter>>,
    contention_recorded: bool,
}

impl<'a, T: ?Sized> Future for RwLockReadResult<'a, T> {
    type Output = Result<RwLockReadGuard<'a, T, Async>, PoisonError<RwLockReadGuard<'a, T, Async>>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.lock.try_acquire_read() {
            if let Some(waiter) = self.waiter.take() {
                self.lock.raw.waiters.cancel_marked(&waiter, || {
                    self.lock.raw.state.fetch_and(!WAITERS, Ordering::Release);
                });
            }
            return Poll::Ready(self.lock.acquired_read());
        }

        if self.contention_recorded {
            return self.poll_registered(cx);
        }
        self.lock.record(EventKind::RwLockReadContention);
        self.contention_recorded = true;
        self.poll_registered(cx)
    }
}

#[expect(
    clippy::type_complexity,
    reason = "polling retains an acquired guard on both successful and poisoned acquisition"
)]
impl<'a, T: ?Sized> RwLockReadResult<'a, T> {
    fn poll_registered(
        &mut self,
        cx: &Context<'_>,
    ) -> Poll<Result<RwLockReadGuard<'a, T, Async>, PoisonError<RwLockReadGuard<'a, T, Async>>>> {
        let lock = self.lock;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if lock.raw.waiters.enqueue_if_needed_marked(
            &waiter,
            || {
                lock.raw.state.fetch_or(WAITERS, Ordering::Release);
            },
            || lock.try_acquire_read(),
            || {
                lock.raw.state.fetch_and(!WAITERS, Ordering::Release);
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
            self.lock.raw.waiters.cancel_marked(waiter, || {
                self.lock.raw.state.fetch_and(!WAITERS, Ordering::Release);
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
    type Output = RwLockWriteGuard<'a, T, Async>;

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
    lock: &'a RwLock<T, Async>,
    waiter: Option<Arc<Waiter>>,
    contention_recorded: bool,
}

impl<'a, T: ?Sized> Future for RwLockWriteResult<'a, T> {
    type Output = Result<RwLockWriteGuard<'a, T, Async>, PoisonError<RwLockWriteGuard<'a, T, Async>>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.lock.try_acquire_write() {
            if let Some(waiter) = self.waiter.take() {
                self.lock.raw.waiters.cancel_marked(&waiter, || {
                    self.lock.raw.state.fetch_and(!WAITERS, Ordering::Release);
                });
            }
            return Poll::Ready(self.lock.acquired_write());
        }

        if self.contention_recorded {
            return self.poll_registered(cx);
        }
        self.lock.record(EventKind::RwLockWriteContention);
        self.contention_recorded = true;
        self.poll_registered(cx)
    }
}

#[expect(
    clippy::type_complexity,
    reason = "polling retains an acquired guard on both successful and poisoned acquisition"
)]
impl<'a, T: ?Sized> RwLockWriteResult<'a, T> {
    fn poll_registered(
        &mut self,
        cx: &Context<'_>,
    ) -> Poll<Result<RwLockWriteGuard<'a, T, Async>, PoisonError<RwLockWriteGuard<'a, T, Async>>>> {
        let lock = self.lock;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if lock.raw.waiters.enqueue_if_needed_marked(
            &waiter,
            || {
                lock.raw.state.fetch_or(WAITERS, Ordering::Release);
            },
            || lock.try_acquire_write(),
            || {
                lock.raw.state.fetch_and(!WAITERS, Ordering::Release);
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
            self.lock.raw.waiters.cancel_marked(waiter, || {
                self.lock.raw.state.fetch_and(!WAITERS, Ordering::Release);
            });
        }
    }
}

/// Shared ownership of an [`RwLock`] using the same synchronization mode.
pub struct RwLockReadGuard<'a, T: ?Sized, M: Mode = Sync> {
    lock: &'a RwLock<T, M>,
    raw: M::RwLockReadGuard<'a>,
    marker: PhantomData<&'a T>,
}

impl<T: ?Sized, M: Mode> Deref for RwLockReadGuard<'_, T, M> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the sealed strategy acquires shared ownership before
        // constructing this guard; that ownership excludes all writers.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized> RwLockReadGuard<'_, T, Async> {
    pub(super) fn release_inner(&self) {
        self.lock.unlock_read();
    }
}

impl<T: ?Sized, M: Mode> Drop for RwLockReadGuard<'_, T, M> {
    fn drop(&mut self) {
        M::rwlock_release_read(self, super::mode::sealed::Release::new());
    }
}

impl<T: ?Sized + fmt::Debug, M: Mode> fmt::Debug for RwLockReadGuard<'_, T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display, M: Mode> fmt::Display for RwLockReadGuard<'_, T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

/// Exclusive ownership of an [`RwLock`] using the same synchronization mode.
pub struct RwLockWriteGuard<'a, T: ?Sized, M: Mode = Sync> {
    lock: &'a RwLock<T, M>,
    raw: M::RwLockWriteGuard<'a>,
    panicking_at_acquisition: bool,
    marker: PhantomData<&'a mut T>,
}

impl<T: ?Sized, M: Mode> Deref for RwLockWriteGuard<'_, T, M> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: only the sealed strategies construct this guard, after
        // acquiring exclusive ownership of the raw lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized, M: Mode> DerefMut for RwLockWriteGuard<'_, T, M> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: exclusive lock ownership excludes other guards, and &mut
        // self excludes simultaneous access through this guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T: ?Sized> RwLockWriteGuard<'_, T, Async> {
    pub(super) fn release_inner(&self) {
        if !self.panicking_at_acquisition && std::thread::panicking() {
            self.lock.poison();
        }
        self.lock.unlock_write();
    }
}

impl<T: ?Sized, M: Mode> Drop for RwLockWriteGuard<'_, T, M> {
    fn drop(&mut self) {
        M::rwlock_release_write(self, super::mode::sealed::Release::new());
    }
}

impl<T: ?Sized + fmt::Debug, M: Mode> fmt::Debug for RwLockWriteGuard<'_, T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display, M: Mode> fmt::Display for RwLockWriteGuard<'_, T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc as StdArc;
    use std::task::{Wake, Waker};

    use super::*;

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: StdArc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn asynchronous_state_starts_unlocked_and_unpoisoned() {
        let state = StateAsync::new();

        assert_eq!(
            (state.state.load(Ordering::Relaxed), state.poisoned.load(Ordering::Relaxed)),
            (0, false),
        );
        drop(state.waiters);
    }

    #[test]
    fn native_try_acquisitions_report_success_and_poison() {
        let lock = RwLock::<_, Sync>::new(0);
        drop(lock.try_write_result_native().unwrap().unwrap());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock.write();
            panic!("poison native writer");
        }));

        drop(lock.try_read_result_native().unwrap_err().into_inner());
        drop(lock.try_write_result_native().unwrap_err().into_inner());
    }

    #[test]
    fn native_guard_release_is_idempotent() {
        let lock = RwLock::<_, Sync>::new(0);
        let mut guard = lock.write();

        guard.release_native();
        guard.release_native();

        assert!(lock.try_write().is_some());
    }

    #[test]
    fn writer_acquires_state_with_registered_waiters() {
        let lock = RwLock::<_, Async>::new(());
        lock.raw.state.store(WAITERS, Ordering::Relaxed);

        assert!(lock.try_acquire_write());
        lock.raw.state.store(0, Ordering::Relaxed);
    }

    #[test]
    fn read_acquisition_preserves_waiter_flags_and_rejects_saturation() {
        let lock = RwLock::<_, Async>::new(());
        let states = [0, WAITERS, WRITER, WAITERS | WRITER, READERS, WAITERS | READERS];
        let actual = states.map(|state| {
            lock.raw.state.store(state, Ordering::Relaxed);
            (lock.try_acquire_read(), lock.raw.state.load(Ordering::Relaxed))
        });

        assert_eq!(
            actual,
            [
                (true, 1),
                (true, WAITERS | 1),
                (false, WRITER),
                (false, WAITERS | WRITER),
                (false, READERS),
                (false, WAITERS | READERS),
            ],
        );
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Failure arms deliberately remain unreachable.
    fn lock_release_during_waiter_registration_completes_acquisition() {
        let lock = RwLock::<_, Async>::new(());
        let context = Context::from_waker(Waker::noop());
        let mut read = lock.read_async_result();
        let Poll::Ready(Ok(read_guard)) = read.poll_registered(&context) else {
            panic!("an unlocked rwlock must complete read registration immediately");
        };
        assert_eq!(lock.raw.state.load(Ordering::Relaxed), 1);
        drop(read_guard);

        let mut write = lock.write_async_result();
        let Poll::Ready(Ok(write_guard)) = write.poll_registered(&context) else {
            panic!("an unlocked rwlock must complete write registration immediately");
        };
        assert_eq!(lock.raw.state.load(Ordering::Relaxed), WRITER);
        drop(write_guard);
    }

    #[test]
    fn last_reader_clears_the_waiter_marker_and_wakes_waiters() {
        let lock = RwLock::<_, Async>::new(());
        lock.raw.state.store(WAITERS | 1, Ordering::Relaxed);
        let waiter = StdArc::new(Waiter::new());
        let counter = StdArc::new(WakeCounter::default());
        waiter.register(&Waker::from(StdArc::clone(&counter)));
        assert!(!lock.raw.waiters.enqueue_if_needed(&waiter, || false));

        lock.unlock_read();

        assert_eq!((lock.raw.state.load(Ordering::Relaxed), counter.0.load(Ordering::Relaxed)), (0, 1));
    }

    #[test]
    fn nonfinal_reader_preserves_the_waiter_marker_without_waking() {
        let lock = RwLock::<_, Async>::new(());
        lock.raw.state.store(WAITERS | 2, Ordering::Relaxed);
        let waiter = StdArc::new(Waiter::new());
        let counter = StdArc::new(WakeCounter::default());
        waiter.register(&Waker::from(StdArc::clone(&counter)));
        assert!(!lock.raw.waiters.enqueue_if_needed(&waiter, || false));

        lock.unlock_read();

        assert_eq!(
            (lock.raw.state.load(Ordering::Relaxed), counter.0.load(Ordering::Relaxed)),
            (WAITERS | 1, 0)
        );
    }

    #[test]
    fn writer_unlock_clears_state_and_wakes_all_waiters() {
        let lock = RwLock::<_, Async>::new(());
        lock.raw.state.store(WAITERS | WRITER, Ordering::Relaxed);
        let first_waiter = StdArc::new(Waiter::new());
        let first = StdArc::new(WakeCounter::default());
        first_waiter.register(&Waker::from(StdArc::clone(&first)));
        assert!(!lock.raw.waiters.enqueue_if_needed(&first_waiter, || false));
        let second_waiter = StdArc::new(Waiter::new());
        let second = StdArc::new(WakeCounter::default());
        second_waiter.register(&Waker::from(StdArc::clone(&second)));
        assert!(!lock.raw.waiters.enqueue_if_needed(&second_waiter, || false));

        lock.unlock_write();

        assert_eq!(
            (
                lock.raw.state.load(Ordering::Relaxed),
                first.0.load(Ordering::Relaxed),
                second.0.load(Ordering::Relaxed),
            ),
            (0, 1, 1)
        );
    }

    #[test]
    fn dropping_pending_futures_removes_the_waiter_marker() {
        let lock = RwLock::<_, Async>::new(());
        let writer = lock.try_write().unwrap();
        let mut context = Context::from_waker(Waker::noop());
        let mut read = Box::pin(lock.read_async_result());
        assert!(read.as_mut().poll(&mut context).is_pending());
        drop(read);
        assert_eq!(lock.raw.state.load(Ordering::Relaxed), WRITER);
        drop(writer);

        let reader = lock.try_read().unwrap();
        let mut write = Box::pin(lock.write_async_result());
        assert!(write.as_mut().poll(&mut context).is_pending());
        drop(write);
        assert_eq!(lock.raw.state.load(Ordering::Relaxed), 1);
        drop(reader);
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Failure arm deliberately remains unreachable.
    fn successful_read_after_registration_clears_only_the_waiter_marker() {
        let lock = RwLock::<_, Async>::new(());
        let _writer = std::mem::ManuallyDrop::new(lock.try_write().unwrap());
        let mut context = Context::from_waker(Waker::noop());
        let mut read = Box::pin(lock.read_async_result());
        assert!(read.as_mut().poll(&mut context).is_pending());
        lock.raw.state.store(WAITERS, Ordering::Release);

        let Poll::Ready(Ok(read_guard)) = read.as_mut().poll(&mut context) else {
            panic!("released writer must allow the registered reader to acquire");
        };

        assert_eq!(lock.raw.state.load(Ordering::Relaxed), 1);
        drop(read_guard);
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Failure arm deliberately remains unreachable.
    fn successful_write_after_registration_clears_only_the_waiter_marker() {
        let lock = RwLock::<_, Async>::new(());
        let _reader = std::mem::ManuallyDrop::new(lock.try_read().unwrap());
        let mut context = Context::from_waker(Waker::noop());
        let mut write = Box::pin(lock.write_async_result());
        assert!(write.as_mut().poll(&mut context).is_pending());
        lock.raw.state.store(WAITERS, Ordering::Release);

        let Poll::Ready(Ok(write_guard)) = write.as_mut().poll(&mut context) else {
            panic!("released reader must allow the registered writer to acquire");
        };

        assert_eq!(lock.raw.state.load(Ordering::Relaxed), WRITER);
        drop(write_guard);
    }

    #[test]
    fn dropping_guards_releases_read_and_write_ownership() {
        let lock = RwLock::<_, Async>::new(());
        let read = lock.try_read().unwrap();
        drop(read);
        assert_eq!(lock.raw.state.load(Ordering::Relaxed), 0);

        let write = lock.try_write().unwrap();
        drop(write);
        assert_eq!(lock.raw.state.load(Ordering::Relaxed), 0);
    }
}
