// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

pub(super) fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

pub(super) fn block_on_timeout<F: Future>(future: F, timeout: Duration) -> Option<F::Output> {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return Some(block_on(future));
    };
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return Some(output),
            Poll::Pending => {
                let remaining = deadline.checked_duration_since(Instant::now())?;
                std::thread::park_timeout(remaining);
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Waiter {
    active: AtomicBool,
    queued: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Waiter {
    pub(super) fn new() -> Self {
        Self {
            active: AtomicBool::new(true),
            queued: AtomicBool::new(false),
            waker: Mutex::new(None),
        }
    }

    pub(super) fn register(&self, waker: &Waker) {
        let mut registered = self.waker.lock().unwrap_or_else(PoisonError::into_inner);
        if registered.as_ref().is_none_or(|registered| !registered.will_wake(waker)) {
            *registered = Some(waker.clone());
        }
    }

    pub(super) fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn take_waker(&self) -> Option<Waker> {
        self.waker.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

#[derive(Debug)]
pub(super) struct WaitQueue {
    has_waiters: AtomicBool,
    waiters: Mutex<VecDeque<Arc<Waiter>>>,
}

impl WaitQueue {
    pub(super) const fn new() -> Self {
        Self {
            has_waiters: AtomicBool::new(false),
            waiters: Mutex::new(VecDeque::new()),
        }
    }

    pub(super) fn enqueue_if_needed(&self, waiter: &Arc<Waiter>, retry: impl FnOnce() -> bool) -> bool {
        self.enqueue_if_needed_marked(waiter, || {}, retry, || {})
    }

    pub(super) fn enqueue_if_needed_marked(
        &self,
        waiter: &Arc<Waiter>,
        mark_waiting: impl FnOnce(),
        retry: impl FnOnce() -> bool,
        clear_waiting: impl FnOnce(),
    ) -> bool {
        let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
        self.has_waiters.store(true, Ordering::Release);
        mark_waiting();
        if retry() {
            waiter.deactivate();
            if waiter.queued.swap(false, Ordering::AcqRel)
                && let Some(index) = waiters.iter().position(|queued| Arc::ptr_eq(queued, waiter))
            {
                waiters.remove(index);
            }
            drop(waiter.take_waker());
            if waiters.is_empty() {
                self.has_waiters.store(false, Ordering::Release);
                clear_waiting();
            }
            return true;
        }
        if !waiter.queued.swap(true, Ordering::AcqRel) {
            waiters.push_back(Arc::clone(waiter));
        }
        false
    }

    pub(super) fn wake_one(&self) {
        if !self.has_waiters.load(Ordering::Acquire) {
            return;
        }
        self.wake_one_marked(|| {});
    }

    pub(super) fn wake_one_marked(&self, clear_waiting: impl FnOnce()) {
        let waker = {
            let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            let waker = loop {
                let Some(waiter) = waiters.pop_front() else {
                    break None;
                };
                waiter.queued.store(false, Ordering::Release);
                if waiter.active.load(Ordering::Acquire) {
                    break waiter.take_waker();
                }
            };
            self.has_waiters.store(!waiters.is_empty(), Ordering::Release);
            if waiters.is_empty() {
                clear_waiting();
            }
            waker
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(super) fn cancel(&self, waiter: &Arc<Waiter>) -> bool {
        self.cancel_marked(waiter, || {})
    }

    pub(super) fn cancel_marked(&self, waiter: &Arc<Waiter>, clear_waiting: impl FnOnce()) -> bool {
        waiter.deactivate();
        let removed = {
            let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            let removed = waiters
                .iter()
                .position(|queued| Arc::ptr_eq(queued, waiter))
                .and_then(|index| waiters.remove(index))
                .is_some();
            if removed {
                waiter.queued.store(false, Ordering::Release);
            }
            if waiters.is_empty() {
                self.has_waiters.store(false, Ordering::Release);
                clear_waiting();
            }
            removed
        };
        drop(waiter.take_waker());
        removed
    }

    pub(super) fn wake_all_marked(&self, clear_waiting: impl FnOnce()) {
        let wakers = {
            let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            let wakers = waiters
                .drain(..)
                .filter_map(|waiter| {
                    waiter.queued.store(false, Ordering::Release);
                    waiter.active.load(Ordering::Acquire).then(|| waiter.take_waker()).flatten()
                })
                .collect::<Vec<_>>();
            self.has_waiters.store(false, Ordering::Release);
            clear_waiting();
            wakers
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::{Duration, Instant};

    use super::{WaitQueue, Waiter, block_on_timeout};

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn retry_removes_an_already_queued_waiter() {
        let queue = WaitQueue::new();
        let waiter = Arc::new(Waiter::new());
        waiter.register(Waker::noop());
        assert!(!queue.enqueue_if_needed(&waiter, || false));

        assert!(queue.enqueue_if_needed(&waiter, || true));
        assert!(!queue.cancel(&waiter));
    }

    #[test]
    fn wake_one_skips_inactive_waiters() {
        let queue = WaitQueue::new();
        let inactive = Arc::new(Waiter::new());
        inactive.register(Waker::noop());
        assert!(!queue.enqueue_if_needed(&inactive, || false));
        inactive.deactivate();

        let active = Arc::new(Waiter::new());
        active.register(Waker::noop());
        assert!(!queue.enqueue_if_needed(&active, || false));
        queue.wake_one();

        assert!(!queue.has_waiters.load(Ordering::Acquire));
    }

    #[test]
    fn wake_one_tolerates_a_stale_waiter_hint() {
        let queue = WaitQueue::new();
        queue.has_waiters.store(true, Ordering::Release);

        queue.wake_one();

        assert!(!queue.has_waiters.load(Ordering::Acquire));
    }

    #[test]
    fn registering_a_new_waker_replaces_the_old_one() {
        let queue = WaitQueue::new();
        let waiter = Arc::new(Waiter::new());
        let first = Arc::new(WakeCounter::default());
        let second = Arc::new(WakeCounter::default());
        waiter.register(&Waker::from(Arc::clone(&first)));
        waiter.register(&Waker::from(Arc::clone(&second)));
        assert!(!queue.enqueue_if_needed(&waiter, || false));

        queue.wake_one();

        assert_eq!((first.0.load(Ordering::Relaxed), second.0.load(Ordering::Relaxed)), (0, 1));
    }

    #[test]
    fn enqueueing_the_same_waiter_twice_keeps_one_queue_entry() {
        let queue = WaitQueue::new();
        let waiter = Arc::new(Waiter::new());
        waiter.register(Waker::noop());

        assert!(!queue.enqueue_if_needed(&waiter, || false));
        assert!(!queue.enqueue_if_needed(&waiter, || false));

        assert_eq!(queue.waiters.lock().unwrap().len(), 1);
    }

    #[test]
    fn wake_all_wakes_every_registered_waiter() {
        let queue = WaitQueue::new();
        let first_waiter = Arc::new(Waiter::new());
        let first = Arc::new(WakeCounter::default());
        first_waiter.register(&Waker::from(Arc::clone(&first)));
        assert!(!queue.enqueue_if_needed(&first_waiter, || false));
        let second_waiter = Arc::new(Waiter::new());
        let second = Arc::new(WakeCounter::default());
        second_waiter.register(&Waker::from(Arc::clone(&second)));
        assert!(!queue.enqueue_if_needed(&second_waiter, || false));

        queue.wake_all_marked(|| {});

        assert_eq!((first.0.load(Ordering::Relaxed), second.0.load(Ordering::Relaxed)), (1, 1));
    }

    #[test]
    fn blocking_executor_is_woken_by_another_thread() {
        struct WakeAfterSpawn {
            ready: Arc<AtomicBool>,
            spawned: bool,
        }

        impl Future for WakeAfterSpawn {
            type Output = ();

            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                if self.ready.load(Ordering::Acquire) {
                    return Poll::Ready(());
                }
                if !self.spawned {
                    self.spawned = true;
                    let ready = Arc::clone(&self.ready);
                    let waker = cx.waker().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(20));
                        ready.store(true, Ordering::Release);
                        waker.wake();
                    });
                }
                Poll::Pending
            }
        }

        let started = Instant::now();
        let completed = block_on_timeout(
            WakeAfterSpawn {
                ready: Arc::new(AtomicBool::new(false)),
                spawned: false,
            },
            Duration::from_secs(1),
        );

        assert_eq!(completed, Some(()));
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
