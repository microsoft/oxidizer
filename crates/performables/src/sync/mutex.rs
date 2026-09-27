// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::task::{Context, Poll};

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::mode::{Async, Mode, Sync};

mod native;

use super::wait_queue::{WaitQueue, Waiter, block_on};
use super::{PoisonError, panic_poisoned};
use crate::telemetry::{self, EventKind};

const LOCKED: u8 = 1;
const WAITERS: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Acquisition {
    Acquired,
    Contended,
}

/// A mutual-exclusion lock with a compile-time synchronization strategy.
///
/// The default [`Sync`] mode uses a compact native blocking mutex. Select
/// [`Async`] to additionally use [`lock_async`](Mutex::lock_async).
///
/// The uncontended path uses atomic operations and does not allocate.
/// The lock is poisoned when an exclusive guard is dropped during an unwind
/// that began after the guard was acquired.
pub struct Mutex<T: ?Sized, M: Mode = Sync> {
    // A native Mutex<T> would require calling a non-const strategy constructor
    // or ambiguous per-mode inherent constructors. Keeping T in one UnsafeCell
    // permits a single const constructor; Sync still retains a real native
    // MutexGuard<()> for ownership, poisoning, and Condvar interoperability.
    raw: M::MutexState,
    value: UnsafeCell<T>,
}

/// Internal asynchronous mutex ownership state.
#[doc(hidden)]
#[derive(Debug)]
pub struct StateAsync {
    state: AtomicU8,
    poisoned: AtomicBool,
    waiters: WaitQueue,
}

impl StateAsync {
    pub(super) const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            poisoned: AtomicBool::new(false),
            waiters: WaitQueue::new(),
        }
    }
}

// SAFETY: ownership of `T` can move with the mutex when `T: Send`.
unsafe impl<T: ?Sized + Send, M: Mode> Send for Mutex<T, M> {}
// SAFETY: access to `T` is serialized by the lock state.
unsafe impl<T: ?Sized + Send, M: Mode> std::marker::Sync for Mutex<T, M> {}

impl<T, M: Mode> Mutex<T, M> {
    /// Creates an unlocked mutex containing `value`.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            raw: M::MUTEX_INIT,
            value: UnsafeCell::new(value),
        }
    }
}

#[expect(clippy::type_complexity, reason = "poison errors retain the acquired mode-specific guard")]
impl<T: ?Sized, M: Mode> Mutex<T, M> {
    /// Blocks until exclusive access is acquired.
    ///
    /// Do not block an executor thread needed to release a conflicting guard.
    ///
    /// # Panics
    ///
    /// Panics after acquisition if the mutex is poisoned.
    pub fn lock(&self) -> MutexGuard<'_, T, M> {
        match self.lock_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Blocks until exclusive access is acquired, returning a usable guard on poison.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] containing the acquired guard when poisoned.
    pub fn lock_result(&self) -> Result<MutexGuard<'_, T, M>, PoisonError<MutexGuard<'_, T, M>>> {
        M::mutex_lock(self)
    }

    /// Attempts to acquire exclusive access without waiting.
    ///
    /// # Panics
    ///
    /// Panics after a successful acquisition if the mutex is poisoned.
    #[must_use]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T, M>> {
        match self.try_lock_result() {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        }
    }

    /// Attempts acquisition without waiting, returning a usable guard on poison.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] containing the acquired guard when poisoned.
    pub fn try_lock_result(&self) -> Result<Option<MutexGuard<'_, T, M>>, PoisonError<MutexGuard<'_, T, M>>> {
        M::mutex_try_lock(self)
    }

    /// Returns whether the mutex is poisoned.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        M::mutex_is_poisoned(self)
    }

    /// Clears poisoning after the protected value has been repaired.
    pub fn clear_poison(&self) {
        M::mutex_clear_poison(self);
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl<T, M: Mode> Mutex<T, M> {
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

#[expect(clippy::type_complexity, reason = "poison errors retain the acquired asynchronous guard")]
impl<T: ?Sized> Mutex<T, Async> {
    /// Returns a future that acquires exclusive access.
    ///
    /// # Panics
    ///
    /// Polling the returned future panics after acquiring the lock if another
    /// thread poisoned it.
    pub fn lock_async(&self) -> MutexLock<'_, T> {
        MutexLock {
            result: self.lock_async_result(),
        }
    }

    /// Returns a future that acquires exclusive access and reports poisoning.
    pub fn lock_async_result(&self) -> MutexLockResult<'_, T> {
        MutexLockResult {
            mutex: self,
            waiter: None,
            contention_recorded: false,
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
    pub(super) fn lock_result_inner(&self) -> Result<MutexGuard<'_, T, Async>, PoisonError<MutexGuard<'_, T, Async>>> {
        if matches!(self.try_acquire(), Acquisition::Acquired) {
            return self.acquired();
        }

        self.record(EventKind::MutexContention);
        block_on(MutexLockResult {
            mutex: self,
            waiter: None,
            contention_recorded: true,
        })
    }

    /// Attempts to acquire exclusive access without waiting and reports poisoning.
    ///
    /// # Errors
    ///
    /// Returns [`PoisonError`] with the acquired guard if the mutex was
    /// successfully acquired after another thread poisoned it.
    pub(super) fn try_lock_result_inner(&self) -> Result<Option<MutexGuard<'_, T, Async>>, PoisonError<MutexGuard<'_, T, Async>>> {
        if matches!(self.try_acquire(), Acquisition::Acquired) {
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
    pub(super) fn is_poisoned_inner(&self) -> bool {
        self.raw.poisoned.load(Ordering::Acquire)
    }

    /// Clears the mutex's poison state after the protected value is repaired.
    pub(super) fn clear_poison_inner(&self) {
        // AcqRel pairs with poison observations and publishes the cleared state
        // to acquisitions that subsequently check it.
        if self.raw.poisoned.swap(false, Ordering::AcqRel) {
            self.record(EventKind::LockPoisonCleared);
        }
    }

    fn try_acquire(&self) -> Acquisition {
        let mut state = self.raw.state.load(Ordering::Relaxed);
        while matches!(state, 0 | WAITERS) {
            match self
                .raw
                .state
                .compare_exchange_weak(state, state.wrapping_add(LOCKED), Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return Acquisition::Acquired,
                Err(current) => state = current,
            }
        }
        Acquisition::Contended
    }

    fn acquired(&self) -> Result<MutexGuard<'_, T, Async>, PoisonError<MutexGuard<'_, T, Async>>> {
        self.record(EventKind::MutexAccess);
        let guard = MutexGuard {
            mutex: self,
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

    fn unlock(&self) {
        let previous = self.raw.state.fetch_sub(LOCKED, Ordering::Release);
        if previous & WAITERS == WAITERS {
            self.raw.waiters.wake_one_marked(|| {
                self.raw.state.fetch_and(!WAITERS, Ordering::Release);
            });
        }
        self.record(EventKind::MutexRelease);
    }
}

impl<T, M: Mode> Mutex<T, M> {
    /// Returns mutable access without locking because the mutex is exclusively borrowed.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: Default, M: Mode> Default for Mutex<T, M> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

#[cfg(feature = "serde")]
impl<T: ?Sized + Serialize, M: Mode> Serialize for Mutex<T, M> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.lock().serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de, T: Deserialize<'de>, M: Mode> Deserialize<'de> for Mutex<T, M> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::new)
    }
}

impl<T: ?Sized + fmt::Debug, M: Mode> fmt::Debug for Mutex<T, M> {
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
    type Output = MutexGuard<'a, T, Async>;

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
    mutex: &'a Mutex<T, Async>,
    waiter: Option<Arc<Waiter>>,
    contention_recorded: bool,
}

impl<'a, T: ?Sized> Future for MutexLockResult<'a, T> {
    type Output = Result<MutexGuard<'a, T, Async>, PoisonError<MutexGuard<'a, T, Async>>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if matches!(self.mutex.try_acquire(), Acquisition::Acquired) {
            if let Some(waiter) = self.waiter.take() {
                self.mutex.raw.waiters.cancel_marked(&waiter, || {
                    self.mutex.raw.state.fetch_and(!WAITERS, Ordering::Release);
                });
            }
            return Poll::Ready(self.mutex.acquired());
        }

        if self.contention_recorded {
            return self.poll_registered(cx);
        }
        self.mutex.record(EventKind::MutexContention);
        self.contention_recorded = true;
        self.poll_registered(cx)
    }
}

#[expect(
    clippy::type_complexity,
    reason = "polling retains an acquired guard on both successful and poisoned acquisition"
)]
impl<'a, T: ?Sized> MutexLockResult<'a, T> {
    fn poll_registered(&mut self, cx: &Context<'_>) -> Poll<Result<MutexGuard<'a, T, Async>, PoisonError<MutexGuard<'a, T, Async>>>> {
        let mutex = self.mutex;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if mutex.raw.waiters.enqueue_if_needed_marked(
            &waiter,
            || {
                mutex.raw.state.fetch_or(WAITERS, Ordering::Release);
            },
            || matches!(mutex.try_acquire(), Acquisition::Acquired),
            || {
                mutex.raw.state.fetch_and(!WAITERS, Ordering::Release);
            },
        ) {
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
            let removed = self.mutex.raw.waiters.cancel_marked(waiter, || {
                self.mutex.raw.state.fetch_and(!WAITERS, Ordering::Release);
            });
            if !removed && self.mutex.raw.state.load(Ordering::Acquire) & LOCKED == 0 {
                self.mutex.raw.waiters.wake_one_marked(|| {
                    self.mutex.raw.state.fetch_and(!WAITERS, Ordering::Release);
                });
            }
        }
    }
}

/// Exclusive ownership of a [`Mutex`] using the same synchronization mode.
///
/// Native [`Sync`] guards cannot be transferred to another thread. [`Async`]
/// guards retain the lock's executor-independent ownership semantics.
pub struct MutexGuard<'a, T: ?Sized, M: Mode = Sync> {
    mutex: &'a Mutex<T, M>,
    raw: M::MutexGuard<'a>,
    panicking_at_acquisition: bool,
    marker: PhantomData<&'a mut T>,
}

impl<T: ?Sized, M: Mode> Deref for MutexGuard<'_, T, M> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: only the sealed strategies construct guards, after acquiring
        // their raw lock. The raw guard is retained for this guard's lifetime.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T: ?Sized, M: Mode> DerefMut for MutexGuard<'_, T, M> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the raw lock proves exclusive access; &mut self prevents
        // simultaneous references through this guard.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<'a, T: ?Sized, M: Mode> MutexGuard<'a, T, M> {
    pub(super) const fn mutex(&self) -> &'a Mutex<T, M> {
        self.mutex
    }
}

impl<T: ?Sized> MutexGuard<'_, T, Async> {
    pub(super) fn release_inner(&self) {
        if !self.panicking_at_acquisition && std::thread::panicking() {
            self.mutex.poison();
        }
        self.mutex.unlock();
    }
}

impl<T: ?Sized, M: Mode> Drop for MutexGuard<'_, T, M> {
    fn drop(&mut self) {
        M::mutex_release(self, super::mode::sealed::Release::new());
    }
}

impl<T: ?Sized + fmt::Debug, M: Mode> fmt::Debug for MutexGuard<'_, T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display, M: Mode> fmt::Display for MutexGuard<'_, T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::AtomicUsize;
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
    #[cfg_attr(coverage_nightly, coverage(off))] // Failure arm deliberately remains unreachable.
    fn unlock_during_waiter_registration_completes_acquisition() {
        let mutex = Mutex::<_, Async>::new(());
        let mut lock = mutex.lock_async_result();
        let context = Context::from_waker(Waker::noop());

        let Poll::Ready(Ok(guard)) = lock.poll_registered(&context) else {
            panic!("an unlocked mutex must complete registration immediately");
        };
        assert_eq!(mutex.raw.state.load(Ordering::Relaxed), LOCKED);
        drop(guard);
    }

    #[test]
    fn acquisition_distinguishes_every_lock_state() {
        let mutex = Mutex::<_, Async>::new(());
        let outcomes = [0, WAITERS, LOCKED, WAITERS | LOCKED].map(|state| {
            mutex.raw.state.store(state, Ordering::Relaxed);
            (mutex.try_acquire(), mutex.raw.state.load(Ordering::Relaxed))
        });

        assert_eq!(
            outcomes,
            [
                (Acquisition::Acquired, LOCKED),
                (Acquisition::Acquired, WAITERS | LOCKED),
                (Acquisition::Contended, LOCKED),
                (Acquisition::Contended, WAITERS | LOCKED),
            ]
        );
    }

    #[test]
    fn unlock_clears_every_lock_state_and_wakes_a_waiter() {
        let mutex = Mutex::<_, Async>::new(());

        mutex.raw.state.store(LOCKED, Ordering::Relaxed);
        mutex.unlock();
        assert_eq!(mutex.raw.state.load(Ordering::Relaxed), 0);

        mutex.raw.state.store(WAITERS | LOCKED, Ordering::Relaxed);
        let waiter = StdArc::new(Waiter::new());
        let counter = StdArc::new(WakeCounter::default());
        waiter.register(&Waker::from(StdArc::clone(&counter)));
        assert!(!mutex.raw.waiters.enqueue_if_needed(&waiter, || false));

        mutex.unlock();

        assert_eq!((mutex.raw.state.load(Ordering::Relaxed), counter.0.load(Ordering::Relaxed)), (0, 1));
    }

    #[test]
    fn dropping_a_selected_waiter_hands_the_unlocked_mutex_to_the_next() {
        let mutex = Mutex::<_, Async>::new(());
        let held = mutex.try_lock().unwrap();
        let first_counter = StdArc::new(WakeCounter::default());
        let first_waker = Waker::from(StdArc::clone(&first_counter));
        let mut first_context = Context::from_waker(&first_waker);
        let second_counter = StdArc::new(WakeCounter::default());
        let second_waker = Waker::from(StdArc::clone(&second_counter));
        let mut second_context = Context::from_waker(&second_waker);
        let mut first = Box::pin(mutex.lock_async_result());
        let mut second = Box::pin(mutex.lock_async_result());
        assert!(first.as_mut().poll(&mut first_context).is_pending());
        assert!(second.as_mut().poll(&mut second_context).is_pending());

        drop(held);
        drop(first);

        assert_eq!(
            (
                mutex.raw.state.load(Ordering::Relaxed),
                first_counter.0.load(Ordering::Relaxed),
                second_counter.0.load(Ordering::Relaxed),
            ),
            (0, 1, 1)
        );
        assert!(second.as_mut().poll(&mut second_context).is_ready());
    }

    #[test]
    fn cancelling_a_queued_waiter_does_not_wake_another_waiter() {
        let mutex = Mutex::<_, Async>::new(());
        let _held = std::mem::ManuallyDrop::new(mutex.try_lock().unwrap());
        let first_counter = StdArc::new(WakeCounter::default());
        let first_waker = Waker::from(StdArc::clone(&first_counter));
        let mut first_context = Context::from_waker(&first_waker);
        let second_counter = StdArc::new(WakeCounter::default());
        let second_waker = Waker::from(StdArc::clone(&second_counter));
        let mut second_context = Context::from_waker(&second_waker);
        let mut first = Box::pin(mutex.lock_async_result());
        let mut second = Box::pin(mutex.lock_async_result());
        assert!(first.as_mut().poll(&mut first_context).is_pending());
        assert!(second.as_mut().poll(&mut second_context).is_pending());
        mutex.raw.state.store(WAITERS, Ordering::Release);

        drop(first);

        assert_eq!(
            (
                mutex.raw.state.load(Ordering::Relaxed),
                first_counter.0.load(Ordering::Relaxed),
                second_counter.0.load(Ordering::Relaxed),
            ),
            (WAITERS, 0, 0)
        );
    }

    #[test]
    fn dropping_a_queued_waiter_clears_only_the_waiter_marker() {
        let mutex = Mutex::<_, Async>::new(());
        let held = mutex.try_lock().unwrap();
        let mut context = Context::from_waker(Waker::noop());
        let mut pending = Box::pin(mutex.lock_async_result());
        assert!(pending.as_mut().poll(&mut context).is_pending());

        drop(pending);

        assert_eq!(mutex.raw.state.load(Ordering::Relaxed), LOCKED);
        drop(held);
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Failure arm deliberately remains unreachable.
    fn successful_acquisition_after_registration_clears_only_the_waiter_marker() {
        let mutex = Mutex::<_, Async>::new(());
        let _held = std::mem::ManuallyDrop::new(mutex.try_lock().unwrap());
        let mut context = Context::from_waker(Waker::noop());
        let mut pending = Box::pin(mutex.lock_async_result());
        assert!(pending.as_mut().poll(&mut context).is_pending());
        mutex.raw.state.store(WAITERS, Ordering::Release);

        let Poll::Ready(Ok(guard)) = pending.as_mut().poll(&mut context) else {
            panic!("released mutex must allow the registered waiter to acquire");
        };

        assert_eq!(mutex.raw.state.load(Ordering::Relaxed), LOCKED);
        drop(guard);
    }

    #[test]
    fn dropping_a_guard_releases_the_mutex() {
        let mutex = Mutex::<_, Async>::new(());
        let guard = mutex.try_lock().unwrap();

        drop(guard);

        assert_eq!(mutex.raw.state.load(Ordering::Relaxed), 0);
    }
}
