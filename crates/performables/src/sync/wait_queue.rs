// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{Thread, current, park, park_timeout};
use std::time::{Duration, Instant};

use crate::sync::raw_mutex::{RawMutex as Mutex, RawMutexGuard};
use crate::sync::slot_list::SlotList;

#[derive(Debug)]
struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

pub(super) fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    #[cfg(test)]
    let deadline = test_deadline();
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                #[cfg(not(test))]
                park();
                #[cfg(test)]
                test_park(deadline);
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
fn test_deadline() -> Option<Instant> {
    (!testing_aids::is_mutation_testing()).then(|| Instant::now() + Duration::from_secs(2))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test watchdog; mutation runs deliberately park without a deadline.
fn test_park(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("test future must make progress before the deadline");
        park_timeout(remaining);
    } else {
        park();
    }
}

pub(super) fn block_on_timeout<F: Future>(future: F, timeout: Duration) -> Option<F::Output> {
    block_on_until(future, Timeout::new(timeout))
}

/// An elapsed-time budget that does not overflow when the duration exceeds Instant's range.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timeout {
    started: Instant,
    duration: Duration,
}

impl Timeout {
    pub(super) fn new(duration: Duration) -> Self {
        Self {
            started: Instant::now(),
            duration,
        }
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub(super) fn remaining_after(&self, elapsed: Duration) -> Option<Duration> {
        let remaining = self.duration.saturating_sub(elapsed);
        // Windows reserves u32::MAX milliseconds for infinity; keep every park finite.
        let maximum_park = Duration::from_millis(u64::from(u32::MAX - 1));
        (!remaining.is_zero()).then_some(remaining.min(maximum_park))
    }

    fn remaining(&self) -> Option<Duration> {
        self.remaining_after(self.elapsed())
    }
}

/// Keeps readiness probes inside the original finite budget; readiness wins at expiry.
pub(super) fn block_on_until<F: Future>(future: F, timeout: Timeout) -> Option<F::Output> {
    block_on_until_with(future, || timeout.remaining())
}

fn block_on_until_with<F: Future>(future: F, mut remaining: impl FnMut() -> Option<Duration>) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(ThreadWaker(current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return Some(output),
            Poll::Pending => {
                park_timeout(remaining()?);
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Waiter {
    active: AtomicBool,
    queued: AtomicBool,
    position: AtomicUsize,
    waker: Mutex<Option<Waker>>,
}

impl Waiter {
    pub(super) fn new() -> Self {
        Self {
            active: AtomicBool::new(true),
            queued: AtomicBool::new(false),
            position: AtomicUsize::new(usize::MAX),
            waker: Mutex::new(None),
        }
    }

    pub(super) fn register(&self, waker: &Waker) {
        // Custom clone/drop hooks may re-enter the primitive being awaited.
        let replacement = waker.clone();
        let previous = self.waker.lock().replace(replacement);
        drop(previous);
    }

    pub(super) fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn take_waker(&self) -> Option<Waker> {
        self.waker.lock().take()
    }
}

#[derive(Debug)]
pub(super) struct WaitQueue {
    state: OnceLock<Box<EagerWaitQueue>>,
    initialization: Mutex<()>,
}

impl WaitQueue {
    pub(super) const fn new() -> Self {
        Self {
            state: OnceLock::new(),
            initialization: Mutex::new(()),
        }
    }

    fn state(&self) -> &EagerWaitQueue {
        if let Some(state) = self.state.get() {
            return state;
        }
        // Empty notifications and first registration share this lock, so a
        // notification cannot miss publication while registration misses it.
        let _initialization = self.initialization.lock();
        self.state.get_or_init(|| Box::new(EagerWaitQueue::new()))
    }

    fn with_existing_state<R>(&self, operation: impl FnOnce(Option<&EagerWaitQueue>) -> R) -> R {
        if let Some(state) = self.state.get() {
            return operation(Some(state));
        }
        let initialization = self.initialization.lock();
        self.with_existing_state_after_initialization(initialization, operation)
    }

    fn with_existing_state_after_initialization<R>(
        &self,
        initialization: RawMutexGuard<'_, ()>,
        operation: impl FnOnce(Option<&EagerWaitQueue>) -> R,
    ) -> R {
        if let Some(state) = self.state.get() {
            drop(initialization);
            operation(Some(state))
        } else {
            // Keep an empty marked clear serialized with first publication.
            operation(None)
        }
    }

    pub(super) fn enqueue_if_needed(&self, waiter: &Arc<Waiter>, retry: impl FnOnce() -> bool) -> bool {
        self.state().enqueue_if_needed(waiter, retry)
    }

    pub(super) fn enqueue_if_needed_marked(
        &self,
        waiter: &Arc<Waiter>,
        mark_waiting: impl FnOnce(),
        retry: impl FnOnce() -> bool,
        clear_waiting: impl FnOnce(),
    ) -> bool {
        self.state().enqueue_if_needed_marked(waiter, mark_waiting, retry, clear_waiting)
    }

    pub(super) fn wake_one(&self) {
        // Conditions need the queue lock to order notification against the
        // generation recheck; the eager channel queue has a separate fast hint.
        self.wake_one_marked(|| {});
    }

    pub(super) fn wake_one_marked(&self, clear_waiting: impl FnOnce()) {
        self.with_existing_state(|state| {
            if let Some(state) = state {
                state.wake_one_marked(clear_waiting);
            } else {
                clear_waiting();
            }
        });
    }

    pub(super) fn cancel(&self, waiter: &Arc<Waiter>) -> bool {
        self.cancel_marked(waiter, || {})
    }

    pub(super) fn cancel_marked(&self, waiter: &Arc<Waiter>, clear_waiting: impl FnOnce()) -> bool {
        waiter.deactivate();
        let removed = self.with_existing_state(|state| {
            if let Some(state) = state {
                state.cancel_marked(waiter, clear_waiting)
            } else {
                clear_waiting();
                false
            }
        });
        drop(waiter.take_waker());
        removed
    }

    pub(super) fn wake_all(&self) {
        self.wake_all_marked(|| {});
    }

    pub(super) fn wake_all_marked(&self, clear_waiting: impl FnOnce()) {
        self.with_existing_state(|state| {
            if let Some(state) = state {
                state.wake_all_marked(clear_waiting);
            } else {
                clear_waiting();
            }
        });
    }
}

#[derive(Debug)]
pub(super) struct EagerWaitQueue {
    has_waiters: AtomicBool,
    waiters: Mutex<SlotList<Arc<Waiter>>>,
}

impl EagerWaitQueue {
    pub(super) const fn new() -> Self {
        Self {
            has_waiters: AtomicBool::new(false),
            waiters: Mutex::new(SlotList::new()),
        }
    }

    #[cfg(test)]
    pub(super) fn has_waiters(&self) -> bool {
        self.has_waiters.load(Ordering::Acquire)
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
        let mut waiters = self.waiters.lock();
        self.has_waiters.store(true, Ordering::Release);
        mark_waiting();
        if retry() {
            waiter.deactivate();
            if waiter.queued.swap(false, Ordering::AcqRel) {
                let index = waiter.position.swap(usize::MAX, Ordering::Relaxed);
                waiters.remove_index_if(index, |queued| Arc::ptr_eq(queued, waiter));
            }
            let discarded = waiter.take_waker();
            if waiters.is_empty() {
                self.has_waiters.store(false, Ordering::Release);
                clear_waiting();
            }
            drop(waiters);
            drop(discarded);
            return true;
        }
        if !waiter.queued.swap(true, Ordering::AcqRel) {
            let key = waiters.push_back(Arc::clone(waiter));
            waiter.position.store(key.index, Ordering::Relaxed);
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
            let mut waiters = self.waiters.lock();
            let waker = loop {
                let Some(waiter) = waiters.pop_front() else {
                    break None;
                };
                waiter.queued.store(false, Ordering::Release);
                waiter.position.store(usize::MAX, Ordering::Relaxed);
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
            let mut waiters = self.waiters.lock();
            let index = waiter.position.swap(usize::MAX, Ordering::Relaxed);
            let removed = waiters.remove_index_if(index, |queued| Arc::ptr_eq(queued, waiter)).is_some();
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

    pub(super) fn wake_all(&self) {
        self.wake_all_marked(|| {});
    }

    pub(super) fn wake_all_marked(&self, clear_waiting: impl FnOnce()) {
        let wakers = {
            let mut waiters = self.waiters.lock();
            let mut wakers = Vec::with_capacity(waiters.len());
            while let Some(waiter) = waiters.pop_front() {
                waiter.queued.store(false, Ordering::Release);
                waiter.position.store(usize::MAX, Ordering::Relaxed);
                if waiter.active.load(Ordering::Acquire)
                    && let Some(waker) = waiter.take_waker()
                {
                    wakers.push(waker);
                }
            }
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::{Duration, Instant};

    use super::{Timeout, WaitQueue, Waiter, block_on, block_on_timeout, block_on_until, block_on_until_with};

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct PendingOnce(bool);

    impl Future for PendingOnce {
        type Output = usize;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0 {
                Poll::Ready(7)
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    #[test]
    fn block_on_allows_a_woken_future_to_make_progress() {
        assert_eq!(block_on(PendingOnce(false)), 7);
    }

    #[test]
    fn an_expired_deadline_still_checks_readiness_without_restarting_the_budget() {
        let deadline = Timeout::new(Duration::ZERO);
        assert_eq!(block_on_until(std::future::ready(7), deadline), Some(7));
        assert_eq!(block_on_until(std::future::pending::<usize>(), deadline), None);
        assert_eq!(
            block_on_until_with(std::future::ready(11), || panic!("readiness must precede expiry")),
            Some(11)
        );
    }

    #[test]
    fn overflowing_budgets_stay_finite_and_expire_using_elapsed_time() {
        let timeout = Timeout::new(Duration::MAX);
        assert!(
            timeout
                .remaining_after(Duration::ZERO)
                .is_some_and(|wait| !wait.is_zero() && wait.as_millis() < u128::from(u32::MAX))
        );
        assert_eq!(timeout.remaining_after(Duration::MAX), None);
        let mut probes = 0;
        let result = block_on_until_with(std::future::pending::<()>(), || {
            probes += 1;
            (probes == 1).then_some(Duration::ZERO)
        });
        assert_eq!((result, probes), (None, 2));
    }

    #[test]
    fn retry_removes_an_already_queued_waiter() {
        let queue = WaitQueue::new();
        let waiter = Arc::new(Waiter::new());
        waiter.register(Waker::noop());
        assert!(!queue.enqueue_if_needed(&waiter, || false));

        assert!(queue.enqueue_if_needed(&waiter, || true));
        assert!(!queue.state().has_waiters.load(Ordering::Acquire));
        assert!(queue.state().waiters.lock().is_empty());
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

        assert!(!queue.state().has_waiters.load(Ordering::Acquire));
    }

    #[test]
    fn wake_one_tolerates_a_stale_waiter_hint() {
        let queue = WaitQueue::new();
        queue.state().has_waiters.store(true, Ordering::Release);

        queue.wake_one();

        assert!(!queue.state().has_waiters.load(Ordering::Acquire));
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

        assert_eq!(queue.state().waiters.lock().len(), 1);
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
    fn idle_unmarked_operations_do_not_allocate_queue_storage() {
        let queue = WaitQueue::new();
        let waiter = Arc::new(Waiter::new());
        waiter.register(Waker::noop());
        queue.wake_one();
        queue.wake_all();
        assert!(!queue.cancel(&waiter));

        assert_eq!(
            (
                queue.state.get().is_none(),
                waiter.active.load(Ordering::Acquire),
                waiter.take_waker().is_none(),
            ),
            (true, false, true),
        );
    }

    #[test]
    fn empty_marked_clears_serialize_with_first_publication_without_allocating() {
        let queue = WaitQueue::new();
        let clears = AtomicUsize::new(0);
        queue.wake_one_marked(|| {
            assert!(queue.initialization.try_lock().is_none());
            clears.fetch_add(1, Ordering::Relaxed);
        });
        queue.wake_all_marked(|| {
            assert!(queue.initialization.try_lock().is_none());
            clears.fetch_add(1, Ordering::Relaxed);
        });
        let waiter = Arc::new(Waiter::new());
        assert!(!queue.cancel_marked(&waiter, || {
            assert!(queue.initialization.try_lock().is_none());
            clears.fetch_add(1, Ordering::Relaxed);
        }));

        assert_eq!(clears.load(Ordering::Relaxed), 3);
        assert!(queue.state.get().is_none());
    }

    #[test]
    fn publication_after_an_empty_operation_starts_is_observed() {
        let queue = WaitQueue::new();
        let initialization = queue.initialization.lock();
        queue.state.set(Box::new(super::EagerWaitQueue::new())).unwrap();

        assert!(queue.with_existing_state_after_initialization(initialization, |state| state.is_some()));
    }

    #[test]
    fn contention_reuses_one_queue_allocation() {
        let queue = WaitQueue::new();
        let first = Arc::new(Waiter::new());
        first.register(Waker::noop());
        assert!(!queue.enqueue_if_needed(&first, || false));
        let storage = std::ptr::from_ref(queue.state());
        queue.wake_one();
        let second = Arc::new(Waiter::new());
        second.register(Waker::noop());
        assert!(!queue.enqueue_if_needed(&second, || false));

        assert_eq!(std::ptr::from_ref(queue.state()), storage);
        queue.wake_one();
    }

    #[test]
    fn blocking_executor_is_woken_by_another_thread() {
        struct WakeAfterSpawn {
            ready: Arc<AtomicBool>,
            spawned: bool,
        }

        impl Future for WakeAfterSpawn {
            type Output = ();

            #[cfg_attr(coverage_nightly, coverage(off))] // Test-driver scheduling edges are not product behavior.
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
