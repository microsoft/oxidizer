// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coalescing notifications.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use super::raw_mutex::RawMutex as Mutex;
use super::slot_list::SlotList;
use super::{Async, Mode, Sync};
use crate::telemetry::{self, EventKind};

const EMPTY: u32 = 0;
const PERMIT: u32 = 1;
const WAITING: u32 = 2;

/// A retained, coalescing notification.
///
/// [`notify_one`](Self::notify_one) delivers one notification to a registered
/// waiter, or stores one permit when nobody is registered. Repeated notifications
/// without waiters coalesce into that single permit. Each successful wait consumes
/// one notification. Notifications assigned to cancelled waits are passed on.
///
/// Unlike [`super::condition::Condvar`], notification before waiting is retained;
/// no mutex guard is required. Use a channel or semaphore to count every event.
///
/// Ready waits and notifications do not allocate; contended waits allocate
/// registration storage.
///
/// ```
/// use performables::sync::notify::Notify;
/// let notify: Notify = Notify::new();
/// notify.notify_one();
/// notify.wait(); // Consumes the retained permit without blocking.
/// assert!(!notify.try_wait());
/// ```
///
/// For multiple consumers, register before checking the shared work queue with
/// [`NotifyWait::enable`]; otherwise two notifications can coalesce before the
/// consumers start waiting.
#[derive(Debug)]
pub struct Notify<M: Mode = Sync> {
    raw: M::NotifyState,
}

/// Internal notification storage selected by a synchronization mode.
#[doc(hidden)]
#[derive(Debug)]
pub struct State {
    state: AtomicU32,
    // A sidecar keeps ready notifications allocation-free and both modes compact.
    queue: OnceLock<Box<Mutex<SlotList<Arc<Waiter>>>>>,
}

#[derive(Debug)]
struct Waiter {
    notified: AtomicBool,
    position: AtomicUsize,
    target: Target,
}

#[derive(Debug)]
enum Target {
    Thread(Thread),
    Task(Mutex<Option<Waker>>),
}

impl Waiter {
    fn wake(&self) {
        match &self.target {
            Target::Thread(thread) => thread.unpark(),
            Target::Task(waker) => {
                let waker = waker.lock().take();
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        }
    }

    fn register(&self, waker: &Waker) {
        let Target::Task(registered) = &self.target else {
            unreachable!("only task registrations replace their waker");
        };
        // Clone outside the lock: a custom waker can re-enter notification.
        let replacement = waker.clone();
        let previous = registered.lock().replace(replacement);
        drop(previous);
    }
}

impl State {
    pub(super) const fn new() -> Self {
        Self {
            state: AtomicU32::new(EMPTY),
            queue: OnceLock::new(),
        }
    }

    #[inline]
    fn try_take(&self) -> bool {
        self.state
            .compare_exchange(PERMIT, EMPTY, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    fn notify_one(&self) {
        self.notify_from(self.state.load(Ordering::Relaxed));
    }

    #[inline]
    fn notify_from(&self, mut state: u32) {
        loop {
            if state == WAITING {
                if self.notify_waiter() {
                    return;
                }
                state = self.state.load(Ordering::Relaxed);
                continue;
            }
            // Even a coalesced notification publishes writes to the consumer.
            match self
                .state
                .compare_exchange_weak(state, PERMIT, Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(current) => state = current,
            }
        }
    }

    #[cold]
    fn notify_waiter(&self) -> bool {
        // The relaxed header read observed WAITING's release publication.
        // Acquire that publication before inspecting the initialized sidecar.
        std::sync::atomic::fence(Ordering::Acquire);
        let queue = self.queue.get().expect("WAITING is published only after queue initialization");
        let mut queue = queue.lock();
        if let Some(waiter) = queue.pop_front() {
            waiter.position.store(usize::MAX, Ordering::Relaxed);
            if queue.is_empty() {
                self.state.store(EMPTY, Ordering::Release);
            }
            waiter.notified.store(true, Ordering::Release);
            drop(queue);
            waiter.wake();
            true
        } else {
            // Cancellation or another notifier drained the queue before this lock.
            false
        }
    }

    fn enqueue(&self, waiter: &Arc<Waiter>) {
        self.enqueue_with(waiter, || {});
    }

    fn enqueue_with(&self, waiter: &Arc<Waiter>, mut before_register: impl FnMut()) {
        let mut queue = self.queue.get_or_init(|| Box::new(Mutex::new(SlotList::new()))).lock();
        loop {
            if self.try_take() {
                waiter.notified.store(true, Ordering::Release);
                return;
            }
            before_register();
            match self.state.compare_exchange(EMPTY, WAITING, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) | Err(WAITING) => {
                    let key = queue.push_back(Arc::clone(waiter));
                    waiter.position.store(key.index, Ordering::Relaxed);
                    return;
                }
                Err(_) => {}
            }
        }
    }

    fn cancel(&self, waiter: &Arc<Waiter>) {
        let mut queue = self
            .queue
            .get()
            .expect("a registration initializes its queue before publication")
            .lock();
        let index = waiter.position.swap(usize::MAX, Ordering::Relaxed);
        if queue.remove_index_if(index, |queued| Arc::ptr_eq(queued, waiter)).is_some() && queue.is_empty() {
            self.state.store(EMPTY, Ordering::Release);
        }
        let notified = waiter.notified.load(Ordering::Acquire);
        drop(queue);
        if notified {
            self.notify_one();
        }
    }
}

impl<M: Mode> Notify<M> {
    /// Creates a notification without an available permit.
    #[must_use]
    pub const fn new() -> Self {
        Self { raw: M::NOTIFY_INIT }
    }

    #[inline]
    fn state(&self) -> &State {
        M::notify_state(&self.raw)
    }

    #[inline]
    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }

    /// Delivers to one registered waiter or retains one permit for a later wait.
    ///
    /// Calling this method never waits for the consumer to run. Registered waits
    /// are selected in registration order; task scheduling can change return order.
    #[inline]
    pub fn notify_one(&self) {
        self.state().notify_one();
        self.record(EventKind::NotifySignal);
    }

    /// Attempts to consume a stored permit without blocking.
    ///
    /// A notification already assigned to another waiter cannot be taken.
    #[inline]
    #[must_use]
    pub fn try_wait(&self) -> bool {
        if self.state().try_take() {
            self.record(EventKind::NotifyAccess);
            true
        } else {
            false
        }
    }

    /// Returns a snapshot of whether an unassigned permit is stored.
    ///
    /// Another waiter can consume it immediately. This is not a substitute for
    /// [`try_wait`](Self::try_wait) or registering before checking shared state.
    #[inline]
    #[must_use]
    pub fn is_notified(&self) -> bool {
        self.state().state.load(Ordering::Relaxed) == PERMIT
    }

    /// Blocks until this wait consumes a notification.
    ///
    /// On an executor thread, use [`Notify::wait_async`] instead to avoid blocking
    /// tasks needed to produce the notification.
    pub fn wait(&self) {
        if !self.try_wait() {
            self.wait_contended(None);
        }
    }

    /// Consumes a notification, blocking for at most `timeout` when none is ready.
    ///
    /// Returns `true` on notification and `false` on timeout. A ready permit wins
    /// even with a zero timeout. Timeout does not discard an assigned notification;
    /// it is passed to another waiter or retained. An overflowing deadline waits
    /// without a deadline.
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.try_wait() {
            true
        } else if timeout.is_zero() {
            self.record(EventKind::NotifyContention);
            false
        } else {
            self.wait_contended(Instant::now().checked_add(timeout))
        }
    }

    #[cold]
    fn wait_contended(&self, deadline: Option<Instant>) -> bool {
        let waiter = Arc::new(Waiter {
            notified: AtomicBool::new(false),
            position: AtomicUsize::new(usize::MAX),
            target: Target::Thread(std::thread::current()),
        });
        self.state().enqueue(&waiter);
        let registration = Registration {
            state: self.state(),
            waiter,
        };
        self.record(EventKind::NotifyContention);
        loop {
            if registration.waiter.notified.load(Ordering::Acquire) {
                // Successful consumption disarms cancellation's notification handoff.
                registration.waiter.notified.store(false, Ordering::Relaxed);
                drop(registration);
                self.record(EventKind::NotifyAccess);
                return true;
            }
            if let Some(deadline) = deadline {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return false;
                };
                std::thread::park_timeout(remaining);
            } else {
                std::thread::park();
            }
        }
    }
}

struct Registration<'a> {
    state: &'a State,
    waiter: Arc<Waiter>,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.state.cancel(&self.waiter);
    }
}

impl<M: Mode> Default for Notify<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl Notify<Async> {
    /// Returns a future that consumes one notification.
    ///
    /// Construction does not register a waiter. Poll the future or call
    /// [`NotifyWait::enable`] to register. Dropping before completion passes an
    /// assigned notification on rather than losing it.
    ///
    /// ```
    /// # async fn example() {
    /// use performables::sync::Async;
    /// use performables::sync::notify::Notify;
    /// let notify = Notify::<Async>::new();
    /// notify.notify_one();
    /// notify.wait_async().await;
    /// # }
    /// ```
    pub fn wait_async(&self) -> NotifyWait<'_> {
        NotifyWait {
            notify: self,
            waiter: None,
            received: false,
            completed: false,
        }
    }
}

/// A notification wait returned by [`Notify::wait_async`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled, awaited, or explicitly enabled"]
pub struct NotifyWait<'a> {
    notify: &'a Notify<Async>,
    waiter: Option<Arc<Waiter>>,
    received: bool,
    completed: bool,
}

impl NotifyWait<'_> {
    /// Registers this wait before checking a separately synchronized work queue.
    ///
    /// Returns whether a notification is already assigned. It is consumed only
    /// when the future completes; dropping an enabled wait passes it on.
    ///
    /// Multiple consumers should enable their waits before checking shared work,
    /// then await if no work is available. Enabling twice does not register twice.
    #[inline]
    pub fn enable(&mut self) -> bool {
        if self.received || self.completed {
            return true;
        }
        if self.waiter.is_none() {
            if self.notify.state().try_take() {
                self.received = true;
                return true;
            }
            self.register_contended();
        }
        self.waiter.as_ref().is_some_and(|waiter| waiter.notified.load(Ordering::Acquire))
    }

    #[cold]
    fn register_contended(&mut self) {
        let waiter = Arc::new(Waiter {
            notified: AtomicBool::new(false),
            position: AtomicUsize::new(usize::MAX),
            target: Target::Task(Mutex::new(None)),
        });
        self.notify.state().enqueue(&waiter);
        self.waiter = Some(waiter);
        self.notify.record(EventKind::NotifyContention);
    }
}

impl Future for NotifyWait<'_> {
    type Output = ();

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.completed {
            return Poll::Ready(());
        }
        if self.enable() {
            self.completed = true;
            self.notify.record(EventKind::NotifyAccess);
            return Poll::Ready(());
        }
        let waiter = self.waiter.as_ref().expect("enable either receives or registers this wait");
        waiter.register(cx.waker());
        if waiter.notified.load(Ordering::Acquire) {
            self.completed = true;
            self.notify.record(EventKind::NotifyAccess);
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for NotifyWait<'_> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if let Some(waiter) = &self.waiter {
            self.notify.state().cancel(waiter);
        } else if self.received {
            self.notify.state().notify_one();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn registration_rechecks_a_permit_and_stale_waiter_hints_retry() {
        let state = State::new();
        let waiter = Arc::new(Waiter {
            notified: AtomicBool::new(false),
            position: AtomicUsize::new(usize::MAX),
            target: Target::Thread(std::thread::current()),
        });
        state.notify_one();
        state.enqueue(&waiter);
        assert!(waiter.notified.load(Ordering::Acquire));
        state.cancel(&waiter);
        assert!(state.try_take());
        assert!(!state.notify_waiter());
        state.notify_from(WAITING);
        assert!(state.try_take());
        state.notify_one();
        state.notify_from(EMPTY);
        assert!(state.try_take());
    }

    #[test]
    fn blocking_registration_is_counted_before_notification_delivery() {
        let notify = Arc::new(Notify::<Sync>::new());
        let worker = std::thread::spawn({
            let notify = Arc::clone(&notify);
            move || {
                notify.wait();
                true
            }
        });
        let deadline = Instant::now() + Duration::from_secs(if cfg!(miri) { 120 } else { 5 });
        while notify.state().state.load(Ordering::Acquire) != WAITING {
            assert!(Instant::now() < deadline, "the blocking waiter must register");
            std::thread::yield_now();
        }
        notify.notify_one();
        assert!(worker.join().unwrap());
        assert!(!notify.is_notified());
    }

    #[test]
    fn notification_between_readiness_probe_and_queue_publication_is_retained() {
        let state = State::new();
        let waiter = Arc::new(Waiter {
            notified: AtomicBool::new(false),
            position: AtomicUsize::new(usize::MAX),
            target: Target::Thread(std::thread::current()),
        });
        state.enqueue_with(&waiter, || state.notify_one());
        assert!(waiter.notified.load(Ordering::Acquire));
        assert!(!state.try_take());
        waiter.notified.store(false, Ordering::Relaxed);
        state.cancel(&waiter);
    }

    #[test]
    #[should_panic(expected = "only task registrations replace their waker")]
    fn thread_waiters_reject_task_waker_registration() {
        let waiter = Waiter {
            notified: AtomicBool::new(false),
            position: AtomicUsize::new(usize::MAX),
            target: Target::Thread(std::thread::current()),
        };
        waiter.register(Waker::noop());
    }
}
