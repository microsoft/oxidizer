// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use super::mutex::{MutexGuard, MutexLock};
use super::wait_queue::{WaitQueue, Waiter, block_on, block_on_timeout};
use crate::telemetry::{self, EventKind};

/// An executor-independent condition variable used with [`super::mutex::Mutex`].
#[derive(Debug)]
pub struct Condvar {
    generation: AtomicU64,
    waiters: WaitQueue,
}

impl Condvar {
    /// Creates a condition variable.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            waiters: WaitQueue::new(),
        }
    }

    /// Releases `guard` and returns a future that reacquires its mutex after notification.
    ///
    /// Callers must re-check their predicate after this returns because condition-variable
    /// waits may complete spuriously.
    pub fn wait<'condition, 'mutex, T: ?Sized>(&'condition self, guard: MutexGuard<'mutex, T>) -> CondvarWait<'condition, 'mutex, T> {
        let mutex = guard.mutex();
        CondvarWait {
            condition: self,
            mutex,
            generation: self.generation.load(Ordering::Acquire),
            guard: Some(guard),
            waiter: None,
            lock: None,
            notified: false,
        }
    }

    /// Releases `guard`, blocks for a notification, and reacquires its mutex.
    pub fn wait_sync<'mutex, T: ?Sized>(&self, guard: MutexGuard<'mutex, T>) -> MutexGuard<'mutex, T> {
        block_on(self.wait(guard))
    }

    /// Waits asynchronously while `condition` returns `true`.
    pub async fn wait_while<'mutex, T: ?Sized, F>(&self, mut guard: MutexGuard<'mutex, T>, mut condition: F) -> MutexGuard<'mutex, T>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait(guard).await;
        }
        guard
    }

    /// Blocks while `condition` returns `true`.
    pub fn wait_while_sync<'mutex, T: ?Sized, F>(&self, mut guard: MutexGuard<'mutex, T>, mut condition: F) -> MutexGuard<'mutex, T>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait_sync(guard);
        }
        guard
    }

    /// Releases `guard`, waits up to `timeout`, and reacquires its mutex.
    pub fn wait_timeout_sync<'mutex, T: ?Sized>(
        &self,
        guard: MutexGuard<'mutex, T>,
        timeout: Duration,
    ) -> (MutexGuard<'mutex, T>, WaitTimeoutResult) {
        let mutex = guard.mutex();
        match block_on_timeout(self.wait(guard), timeout) {
            Some(guard) => (guard, WaitTimeoutResult { timed_out: false }),
            None => (mutex.lock_sync(), WaitTimeoutResult { timed_out: true }),
        }
    }

    /// Wakes one waiting task or thread.
    pub fn notify_one(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.record(EventKind::CondvarNotify);
        self.waiters.wake_one();
    }

    /// Wakes all waiting tasks and threads.
    pub fn notify_all(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.record(EventKind::CondvarNotify);
        self.waiters.wake_all_marked(|| {});
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}

/// A future returned by [`Condvar::wait`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct CondvarWait<'condition, 'mutex, T: ?Sized> {
    condition: &'condition Condvar,
    mutex: &'mutex super::mutex::Mutex<T>,
    generation: u64,
    guard: Option<MutexGuard<'mutex, T>>,
    waiter: Option<Arc<Waiter>>,
    lock: Option<MutexLock<'mutex, T>>,
    notified: bool,
}

impl<'mutex, T: ?Sized> Future for CondvarWait<'_, 'mutex, T> {
    type Output = MutexGuard<'mutex, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.notified {
            let released = self.guard.take().is_some();

            if self.condition.generation.load(Ordering::Acquire) == self.generation {
                if released {
                    self.condition.record(EventKind::CondvarContention);
                }
                let condition = self.condition;
                let generation = self.generation;
                let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
                waiter.register(cx.waker());
                if !condition
                    .waiters
                    .enqueue_if_needed(&waiter, || condition.generation.load(Ordering::Acquire) != generation)
                {
                    return Poll::Pending;
                }
                if let Some(waiter) = self.waiter.take() {
                    condition.waiters.cancel(&waiter);
                }
            }

            self.notified = true;
            if self.lock.is_none() {
                self.lock = Some(self.mutex.lock());
            }
        }

        let lock = self.lock.as_mut().expect("notified waits always reacquire their mutex");
        match Pin::new(lock).poll(cx) {
            Poll::Ready(guard) => {
                self.condition.record(EventKind::CondvarAccess);
                Poll::Ready(guard)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: ?Sized> Drop for CondvarWait<'_, '_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            self.condition.waiters.cancel(waiter);
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
