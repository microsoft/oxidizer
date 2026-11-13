// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coalescing notifications.

use std::pin::Pin;
use std::ptr::from_ref;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};
use std::thread::{Thread, current, park, park_timeout};
use std::time::Duration;

use crate::sync::effects::fence;
use crate::sync::raw_mutex::RawMutex as Mutex;
use crate::sync::slot_list::SlotList;
use crate::sync::wait_queue::Timeout;
use crate::sync::{Async, Mode, Sync};
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
/// Ready waits and notifications allocate no primitive-owned waiter storage;
/// contended waits allocate registration storage. Async delivery invokes the
/// registered task waker, whose code may allocate, re-enter or panic. Telemetry
/// recording has its own allocation behavior.
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

    fn try_take(&self) -> bool {
        self.state
            .compare_exchange(PERMIT, EMPTY, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn notify_one(&self) {
        self.notify_from(self.state.load(Ordering::Relaxed));
    }

    // #[gamma::skip(parameter.default_shadow, reason = "The initial state is only a CAS hint; a stale EMPTY reloads the actual state through the failed CAS before deciding whether to deliver or store a permit.")]
    fn notify_from(&self, state: u32) {
        self.notify_from_with(state, || self.notify_waiter());
    }

    // #[gamma::skip(parameter.default_shadow, reason = "The initial state is only a CAS hint; the failed CAS reloads a current WAITING or PERMIT state.")]
    fn notify_from_with(&self, mut state: u32, mut notify_waiter: impl FnMut() -> bool) {
        loop {
            if state == WAITING {
                if
                // #[gamma::skip(cond.always_false, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                notify_waiter() {
                    return;
                }
                // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                // #[gamma::skip(assign_value.default, reason = "EMPTY is a valid stale hint after failed waiter delivery; the next CAS either publishes the permit or reloads current state.")]
                state = self.state.load(Ordering::Relaxed);
                continue;
            }
            // Even a coalesced notification publishes writes to the consumer.
            match self.state.compare_exchange_weak(
                // #[gamma::skip(expr.increment, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                state,
                PERMIT,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(current) => {
                    // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "The November 4 afternoon campaign timed out because removing this update repeatedly retried compare_exchange with stale state.")]
                    state =
                // #[gamma::skip(assign_value.default, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                current;
                }
            }
        }
    }

    // #[gamma::skip(fn_value.bool_false, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    #[cold]
    fn notify_waiter(&self) -> bool {
        // The relaxed header read observed WAITING's release publication.
        // Acquire that publication before inspecting the initialized sidecar.
        fence(Ordering::Acquire);
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
            if
            // #[gamma::skip(cond.always_false, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            self.try_take() {
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

    fn state(&self) -> &State {
        M::notify_state(&self.raw)
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, from_ref(self).cast::<()>());
    }

    /// Delivers to one registered waiter or retains one permit for a later wait.
    ///
    /// Calling this method never waits for the consumer to run. Registered waits
    /// are selected in registration order; task scheduling can change return order.
    #[inline]
    pub fn notify_one(&self) {
        self.record(EventKind::NotifySignal);
        self.state().notify_one();
    }

    /// Attempts to consume a stored permit without blocking.
    ///
    /// A notification already assigned to another waiter cannot be taken.
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
    /// it is passed to another waiter or retained. The timeout remains finite
    /// even when its duration cannot be added to an [`std::time::Instant`].
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        self.wait_timeout_with(timeout, |deadline| self.wait_contended(deadline))
    }

    fn wait_timeout_with(&self, timeout: Duration, wait: impl FnOnce(Option<Timeout>) -> bool) -> bool {
        if self.try_wait() {
            true
        } else if timeout.is_zero() {
            self.record(EventKind::NotifyContention);
            false
        } else {
            wait(
                // #[gamma::skip(option.some_to_none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                Some(Timeout::new(timeout)),
            )
        }
    }

    // #[gamma::skip(parameter.default_shadow, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    #[cold]
    fn wait_contended(&self, deadline: Option<Timeout>) -> bool {
        self.wait_contended_with(deadline, Timeout::elapsed, |remaining| match remaining {
            Some(remaining) => park_timeout(remaining),
            None => park(),
        })
    }

    fn wait_contended_with(
        &self,
        deadline: Option<Timeout>,
        mut elapsed: impl FnMut(&Timeout) -> Duration,
        mut park: impl FnMut(Option<Duration>),
    ) -> bool {
        let waiter = Arc::new(Waiter {
            notified: AtomicBool::new(false),
            // #[gamma::skip(expr.decrement, reason = "enqueue overwrites the position if linked; if a permit wins the race both maximum indices remain outside any representable Vec capacity.")]
            position: AtomicUsize::new(usize::MAX),
            target: Target::Thread(current()),
        });
        self.state().enqueue(&waiter);
        let registration = Registration {
            state: self.state(),
            waiter,
        };
        self.record(EventKind::NotifyContention);
        // #[gamma::skip(bool_expr.negate, tag = "equivalent", reason = "Every loop exit returns from the enclosing function, so negating the loop result can never run.")]
        loop {
            if registration.waiter.notified.load(Ordering::Acquire) {
                // Successful consumption disarms cancellation's notification handoff.
                registration.waiter.notified.store(false, Ordering::Relaxed);
                drop(registration);
                self.record(EventKind::NotifyAccess);
                return true;
            }
            if let Some(deadline) = deadline {
                let Some(remaining) = deadline.remaining_after(elapsed(&deadline)) else {
                    return false;
                };
                // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                park(Some(remaining));
            } else {
                // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                park(None);
            }
        }
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
        // #[gamma::skip(logical.or_remove_right, reason = "A completed wait retains either received=true or its notified waiter, so the subsequent checks still return true without registering again.")]
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

struct Registration<'a> {
    state: &'a State,
    waiter: Arc<Waiter>,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.state.cancel(&self.waiter);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn nov4_injected_native_wait_distinguishes_delivery_from_expiry() {
        let notify = Notify::<Sync>::new();
        let parks = Cell::new(0);
        assert!(notify.wait_contended_with(
            None,
            |_| panic!("an indefinite wait must not sample a timeout"),
            |_| {
                parks.set(parks.get() + 1);
                assert_eq!(parks.get(), 1);
                notify.notify_one();
            },
        ));
        assert_eq!(parks.get(), 1);
        assert!(!notify.try_wait());
        assert!(!notify.wait_contended_with(
            Some(Timeout::new(Duration::ZERO)),
            |_| Duration::ZERO,
            |_| panic!("an expired wait must not park"),
        ));
        assert!(!notify.try_wait());
        assert!(notify.state().queue.get().unwrap().lock().is_empty());
    }

    #[test]
    fn gamma_round2_notification_retries_waiter_delivery_instead_of_overwriting_a_new_waiting_registration() {
        let state = super::State::new();
        state.state.store(super::WAITING, Ordering::Relaxed);
        let mut deliveries = 0;
        state.notify_from_with(super::WAITING, || {
            deliveries += 1;
            assert!(deliveries <= 2);
            if deliveries == 1 {
                state.state.store(super::WAITING, Ordering::Relaxed);
                false
            } else {
                state.state.store(super::EMPTY, Ordering::Relaxed);
                true
            }
        });
        assert_eq!(deliveries, 2);
        assert_eq!(state.state.load(Ordering::Relaxed), super::EMPTY);
    }

    #[test]
    fn gamma_round2_timeout_dispatch_only_skips_waiting_for_an_empty_zero_budget() {
        for budget in [Duration::ZERO, Duration::from_millis(7)] {
            let notify = Notify::<Sync>::new();
            let mut calls = 0;
            let completed = notify.wait_timeout_with(budget, |deadline| {
                calls += 1;
                assert_eq!(deadline.unwrap().remaining_after(Duration::ZERO), Some(budget));
                true
            });
            assert_eq!(completed, !budget.is_zero());
            assert_eq!(calls, usize::from(!budget.is_zero()));
        }
        let notify = Notify::<Sync>::new();
        notify.notify_one();
        assert!(notify.wait_timeout_with(Duration::ZERO, |_| panic!("an available permit must not enter the wait path")));
    }

    #[test]
    fn gamma_round2_injected_parking_preserves_success_after_a_spurious_return() {
        let notify = super::Notify::<super::Async>::new();
        let mut parks = 0;
        let received = notify.wait_contended_with(
            None,
            |_| panic!("an unbounded wait does not sample elapsed time"),
            |remaining| {
                assert!(remaining.is_none());
                parks += 1;
                assert!(parks <= 2);
                if parks == 2 {
                    notify.notify_one();
                }
            },
        );
        assert!(received);
        assert_eq!(parks, 2);
        assert!(!notify.try_wait());
    }

    use std::cell::Cell;
    use std::time::Instant;

    use super::*;

    #[test]
    fn gamma_survivor_notify_stale_cached_states_reload_after_cancel_and_failed_compare_exchange() {
        for retained in [false, true] {
            let notify = Notify::<Async>::new();
            let mut wait = notify.wait_async();
            assert!(Pin::new(&mut wait).poll(&mut Context::from_waker(Waker::noop())).is_pending());
            drop(wait);
            if retained {
                notify.notify_one();
            }
            notify.state().notify_from(WAITING);
            assert!(notify.try_wait());
            assert!(!notify.try_wait());
            notify.notify_one();
            notify.state().notify_from(EMPTY);
            assert!(notify.try_wait());
            assert!(!notify.try_wait());
        }
    }

    #[test]
    fn gamma_survivor_native_notification_uses_injected_park_and_disarms_consumed_registration() {
        #[cfg(feature = "seismograph")]
        crate::gamma_round2_telemetry::enable_recording();
        #[cfg(feature = "seismograph")]
        let count = crate::gamma_round2_telemetry::recorded_test_events;
        #[cfg(feature = "seismograph")]
        let before = (
            count(seismograph::recorder::event::EventKind::NotifyContention),
            count(seismograph::recorder::event::EventKind::NotifyAccess),
        );
        let notify = Notify::<Sync>::new();
        let mut parked = 0;
        assert!(notify.wait_contended_with(
            None,
            |_| panic!("an untimed wait must not query elapsed time"),
            |remaining| {
                assert!(remaining.is_none());
                parked += 1;
                assert_eq!(parked, 1);
                notify.notify_one();
            }
        ));
        assert_eq!(parked, 1);
        assert!(
            !notify.try_wait(),
            "a consumed registration must not forward its notification as a new permit"
        );
        assert!(notify.state().queue.get().unwrap().lock().is_empty());
        assert_eq!(notify.state().state.load(Ordering::Relaxed), EMPTY);
        #[cfg(feature = "seismograph")]
        assert_eq!(
            (
                count(seismograph::recorder::event::EventKind::NotifyContention) - before.0,
                count(seismograph::recorder::event::EventKind::NotifyAccess) - before.1,
            ),
            (1, 1)
        );
    }

    #[test]
    fn gamma_survivor_notify_queue_metadata_resets_after_selection_and_last_cancellation() {
        let state = State::new();
        let waiter = || {
            Arc::new(Waiter {
                notified: AtomicBool::new(false),
                position: AtomicUsize::new(usize::MAX),
                target: Target::Task(Mutex::new(Some(Waker::noop().clone()))),
            })
        };
        let first = waiter();
        let second = waiter();
        state.enqueue(&first);
        state.enqueue(&second);
        assert_eq!(state.state.load(Ordering::Acquire), WAITING);
        assert!(!state.try_take());
        state.notify_one();
        assert!(first.notified.load(Ordering::Acquire));
        assert_eq!(first.position.load(Ordering::Relaxed), usize::MAX);
        assert!(!second.notified.load(Ordering::Acquire));
        assert_eq!(state.state.load(Ordering::Acquire), WAITING);
        state.cancel(&second);
        assert_eq!(second.position.load(Ordering::Relaxed), usize::MAX);
        assert_eq!(state.state.load(Ordering::Acquire), EMPTY);
        state.cancel(&first);
        assert!(state.try_take());
        assert!(!state.try_take());
        let reserved = waiter();
        state.notify_one();
        state.enqueue(&reserved);
        assert!(reserved.notified.load(Ordering::Acquire));
        assert_eq!(state.state.load(Ordering::Acquire), EMPTY);
    }

    #[test]
    fn gamma_survivor_notify_racing_permit_registration_keeps_unqueued_position_sentinel() {
        let notify = Notify::<Async>::new();
        notify.notify_one();
        let mut future = notify.wait_async();
        future.register_contended();
        let waiter = future.waiter.as_ref().unwrap();
        assert!(waiter.notified.load(Ordering::Acquire));
        assert_eq!(waiter.position.load(Ordering::Relaxed), usize::MAX);
        assert!(future.enable());
        assert!(Pin::new(&mut future).poll(&mut Context::from_waker(Waker::noop())).is_ready());
        assert!(!notify.try_wait());
    }

    #[test]
    fn forced_expiry_removes_a_registered_wait_without_losing_later_notifications() {
        for duration in [Duration::from_millis(1), Duration::MAX] {
            let notify = Notify::<Sync>::new();
            let elapsed = Cell::new(Duration::ZERO);
            let parks = Cell::new(0);
            assert!(!notify.wait_contended_with(
                Some(Timeout::new(duration)),
                |_| elapsed.get(),
                |remaining| {
                    assert_eq!(notify.state().queue.get().unwrap().lock().len(), 1);
                    assert!(remaining.is_some_and(|wait| !wait.is_zero() && wait <= duration));
                    parks.set(parks.get() + 1);
                    elapsed.set(duration);
                },
            ));
            assert_eq!(parks.get(), 1);
            assert!(notify.state().queue.get().unwrap().lock().is_empty());
            notify.notify_one();
            assert!(notify.try_wait());
            assert!(!notify.try_wait());
        }
    }

    #[test]
    fn spurious_wakes_do_not_restart_the_registered_wait_budget() {
        let notify = Notify::<Sync>::new();
        let mut elapsed = [Duration::from_millis(4), Duration::from_millis(7), Duration::from_millis(10)].into_iter();
        let mut waits = Vec::new();
        assert!(!notify.wait_contended_with(
            Some(Timeout::new(Duration::from_millis(10))),
            |_| elapsed.next().unwrap(),
            |remaining| {
                assert_eq!(notify.state().queue.get().unwrap().lock().len(), 1);
                waits.push(remaining.unwrap());
            },
        ));
        assert_eq!(waits, [Duration::from_millis(6), Duration::from_millis(3)]);
        assert!(notify.state().queue.get().unwrap().lock().is_empty());
    }

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
        let deadline =
            (!testing_aids::is_mutation_testing()).then(|| Instant::now() + Duration::from_secs(if cfg!(miri) { 120 } else { 5 }));
        while notify.state().state.load(Ordering::Acquire) != WAITING {
            if let Some(deadline) = deadline {
                assert!(Instant::now() < deadline, "the blocking waiter must register");
            }
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
