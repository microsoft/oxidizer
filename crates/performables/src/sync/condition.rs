// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use super::mode::{Async, Mode, Sync};
use super::mutex::{MutexGuard, MutexLock};
use super::panic_poisoned;
use super::wait_queue::{WaitQueue, Waiter, block_on, block_on_timeout};
use crate::telemetry::{self, EventKind};

/// A condition variable used with a [`super::mutex::Mutex`] of the same mode.
///
/// The default [`Sync`] mode uses the native condition-variable protocol.
/// Select [`Async`] to also support executor-independent asynchronous waits.
#[derive(Debug)]
pub struct Condvar<M: Mode = Sync> {
    raw: M::CondvarState,
}

/// Internal asynchronous condition-variable state.
#[doc(hidden)]
#[derive(Debug)]
pub struct StateAsync {
    generation: AtomicU64,
    waiters: WaitQueue,
}

impl StateAsync {
    /// Creates a condition variable.
    #[must_use]
    pub(super) const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            waiters: WaitQueue::new(),
        }
    }
}

impl<M: Mode> Condvar<M> {
    /// Creates a condition variable with the selected synchronization mode.
    #[must_use]
    pub const fn new() -> Self {
        Self { raw: M::CONDVAR_INIT }
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl Condvar<Sync> {
    /// Releases `guard`, waits for notification, and reacquires the mutex.
    ///
    /// Always recheck the predicate: native waits may complete spuriously.
    /// As with [`std::sync::Condvar`], use one mutex with each condition variable.
    ///
    /// # Panics
    ///
    /// Panics if the reacquired mutex is poisoned, or the native backend rejects
    /// use with more than one mutex.
    pub fn wait<'mutex, T: ?Sized>(&self, guard: MutexGuard<'mutex, T>) -> MutexGuard<'mutex, T> {
        let mutex = guard.mutex();
        self.record(EventKind::CondvarContention);
        let result = self.raw.wait(guard.into_native());
        let guard = match mutex.acquired_native(result) {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        };
        self.record(EventKind::CondvarAccess);
        guard
    }

    /// Blocks while `condition` returns `true`, checking it with the mutex held.
    ///
    /// # Panics
    ///
    /// Has the same panic conditions as [`wait`](Self::wait).
    pub fn wait_while<'mutex, T: ?Sized, F>(&self, mut guard: MutexGuard<'mutex, T>, mut condition: F) -> MutexGuard<'mutex, T>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait(guard);
        }
        guard
    }

    /// Waits up to `timeout`, always reacquiring the mutex before returning.
    ///
    /// Acquiring the mutex again can finish after the deadline. A timed-out wait does not
    /// report a notification, and a non-timeout can still be spurious.
    ///
    /// # Panics
    ///
    /// Has the same panic conditions as [`wait`](Self::wait).
    #[cfg_attr(test, mutants::skip)] // Timeout-result mutations require scheduler timing and can strand a waiting test.
    pub fn wait_timeout<'mutex, T: ?Sized>(
        &self,
        guard: MutexGuard<'mutex, T>,
        timeout: Duration,
    ) -> (MutexGuard<'mutex, T>, WaitTimeoutResult) {
        let mutex = guard.mutex();
        self.record(EventKind::CondvarContention);
        let (result, outcome) = match self.raw.wait_timeout(guard.into_native(), timeout) {
            Ok((raw, outcome)) => (Ok(raw), outcome),
            Err(error) => {
                let (raw, outcome) = error.into_inner();
                (Err(std::sync::PoisonError::new(raw)), outcome)
            }
        };
        let guard = match mutex.acquired_native(result) {
            Ok(guard) => guard,
            Err(error) => panic_poisoned(&error),
        };
        if !outcome.timed_out() {
            self.record(EventKind::CondvarAccess);
        }
        (
            guard,
            WaitTimeoutResult {
                timed_out: outcome.timed_out(),
            },
        )
    }

    /// Wakes one waiting thread.
    #[inline]
    pub fn notify_one(&self) {
        self.record(EventKind::CondvarNotify);
        self.raw.notify_one();
    }

    /// Wakes all waiting threads.
    #[inline]
    pub fn notify_all(&self) {
        self.record(EventKind::CondvarNotify);
        self.raw.notify_all();
    }
}

impl Condvar<Async> {
    /// Releases `guard` and returns a future that reacquires its mutex after notification.
    ///
    /// Callers must re-check their predicate after this returns because condition-variable
    /// waits may complete spuriously.
    pub fn wait_async<'condition, 'mutex, T: ?Sized>(
        &'condition self,
        guard: MutexGuard<'mutex, T, Async>,
    ) -> CondvarWait<'condition, 'mutex, T> {
        let mutex = guard.mutex();
        CondvarWait {
            condition: self,
            mutex,
            generation: self.raw.generation.load(Ordering::Acquire),
            guard: Some(guard),
            waiter: None,
            lock: None,
            notified: false,
        }
    }

    /// Releases `guard`, blocks for a notification, and reacquires its mutex.
    pub fn wait<'mutex, T: ?Sized>(&self, guard: MutexGuard<'mutex, T, Async>) -> MutexGuard<'mutex, T, Async> {
        block_on(self.wait_async(guard))
    }

    /// Waits asynchronously while `condition` returns `true`.
    pub async fn wait_while_async<'mutex, T: ?Sized, F>(
        &self,
        mut guard: MutexGuard<'mutex, T, Async>,
        mut condition: F,
    ) -> MutexGuard<'mutex, T, Async>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait_async(guard).await;
        }
        guard
    }

    /// Blocks while `condition` returns `true`.
    pub fn wait_while<'mutex, T: ?Sized, F>(
        &self,
        mut guard: MutexGuard<'mutex, T, Async>,
        mut condition: F,
    ) -> MutexGuard<'mutex, T, Async>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait(guard);
        }
        guard
    }

    /// Releases `guard`, waits up to `timeout`, and reacquires its mutex.
    pub fn wait_timeout<'mutex, T: ?Sized>(
        &self,
        guard: MutexGuard<'mutex, T, Async>,
        timeout: Duration,
    ) -> (MutexGuard<'mutex, T, Async>, WaitTimeoutResult) {
        let mutex = guard.mutex();
        match block_on_timeout(self.wait_async(guard), timeout) {
            Some(guard) => (guard, WaitTimeoutResult { timed_out: false }),
            None => (mutex.lock(), WaitTimeoutResult { timed_out: true }),
        }
    }

    /// Wakes one waiting task or thread.
    pub fn notify_one(&self) {
        self.raw.generation.fetch_add(1, Ordering::Release);
        self.record(EventKind::CondvarNotify);
        self.raw.waiters.wake_one();
    }

    /// Wakes all waiting tasks and threads.
    pub fn notify_all(&self) {
        self.raw.generation.fetch_add(1, Ordering::Release);
        self.record(EventKind::CondvarNotify);
        self.raw.waiters.wake_all();
    }
}

impl<M: Mode> Default for Condvar<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// A future returned by [`Condvar::wait_async`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct CondvarWait<'condition, 'mutex, T: ?Sized> {
    condition: &'condition Condvar<Async>,
    mutex: &'mutex super::mutex::Mutex<T, Async>,
    generation: u64,
    guard: Option<MutexGuard<'mutex, T, Async>>,
    waiter: Option<Arc<Waiter>>,
    lock: Option<MutexLock<'mutex, T>>,
    notified: bool,
}

impl<'mutex, T: ?Sized> Future for CondvarWait<'_, 'mutex, T> {
    type Output = MutexGuard<'mutex, T, Async>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.notified {
            let released = self.guard.take().is_some();

            if self.condition.raw.generation.load(Ordering::Acquire) == self.generation {
                if released {
                    self.condition.record(EventKind::CondvarContention);
                }
                let condition = self.condition;
                let generation = self.generation;
                let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
                waiter.register(cx.waker());
                if !condition
                    .raw
                    .waiters
                    .enqueue_if_needed(&waiter, || condition.raw.generation.load(Ordering::Acquire) != generation)
                {
                    return Poll::Pending;
                }
                if let Some(waiter) = self.waiter.take() {
                    condition.raw.waiters.cancel(&waiter);
                }
            }

            self.notified = true;
            if self.lock.is_none() {
                self.lock = Some(self.mutex.lock_async());
            }
        }

        let lock = self.lock.as_mut().expect("notified waits always reacquire their mutex");
        match Pin::new(lock).poll(cx) {
            Poll::Ready(guard) => {
                self.waiter.take();
                self.condition.record(EventKind::CondvarAccess);
                Poll::Ready(guard)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: ?Sized> Drop for CondvarWait<'_, '_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter
            && !self.condition.raw.waiters.cancel(waiter)
        {
            self.condition.raw.waiters.wake_one();
        }
    }
}

/// Indicates whether a timed condition-variable wait reached its deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitTimeoutResult {
    timed_out: bool,
}

impl WaitTimeoutResult {
    /// Returns whether the wait reached its deadline before observing a notification.
    #[must_use]
    pub const fn timed_out(self) -> bool {
        self.timed_out
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Wake, Waker};

    use super::*;
    use crate::sync::mutex::Mutex;

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: StdArc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn asynchronous_state_starts_without_notifications_or_waiters() {
        let state = StateAsync::new();

        assert_eq!(state.generation.load(Ordering::Relaxed), 0);
        drop(state.waiters);
    }

    #[test]
    fn notify_one_advances_generation_and_wakes_a_waiter() {
        let mutex = Mutex::<_, Async>::new(());
        let condition = Condvar::<Async>::new();
        let counter = StdArc::new(WakeCounter::default());
        let waker = Waker::from(StdArc::clone(&counter));
        let mut context = Context::from_waker(&waker);
        let mut wait = Box::pin(condition.wait_async(mutex.lock()));
        assert!(wait.as_mut().poll(&mut context).is_pending());

        condition.notify_one();

        assert_eq!(
            (
                condition.raw.generation.load(Ordering::Relaxed),
                counter.0.load(Ordering::Relaxed),
                wait.as_mut().poll(&mut context).is_ready(),
            ),
            (1, 1, true)
        );
    }

    #[test]
    fn notified_wait_reacquires_only_after_the_mutex_is_available() {
        let mutex = Mutex::<_, Async>::new(());
        let condition = Condvar::<Async>::new();
        let mut context = Context::from_waker(Waker::noop());
        let mut wait = Box::pin(condition.wait_async(mutex.lock()));
        assert!(wait.as_mut().poll(&mut context).is_pending());
        let held = mutex.lock();

        condition.notify_one();
        assert!(wait.as_mut().poll(&mut context).is_pending());
        drop(held);

        assert!(wait.as_mut().poll(&mut context).is_ready());
    }
}
