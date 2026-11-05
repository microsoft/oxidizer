// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU8};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::thread::ThreadId;

use performables::sync::channel;

use crate::{RemoteTaskHandle, TaskPtr, executor_context};

const TASK_ACTIVE: u8 = 0b001;
const TASK_ACTIVE_LOCAL: u8 = 0b011;
const TASK_ACTIVE_REMOTE: u8 = 0b101;

#[derive(Clone, Debug)]
pub(crate) struct WakeShared {
    inner: Arc<WakeSharedInner>,
}

#[derive(Debug)]
struct WakeSharedInner {
    thread_id: ThreadId,
    parent_waker: Waker,
    remote_wake_tx: channel::Sender<RemoteWake>,
    remote_wakes_pending: AtomicBool,
}

impl WakeShared {
    pub(crate) fn new(parent_waker: Waker, thread_id: ThreadId) -> (Self, channel::Receiver<RemoteWake>) {
        let (remote_wake_tx, remote_wake_rx) = channel::unbounded();
        let shared = Self {
            inner: Arc::new(WakeSharedInner {
                thread_id,
                parent_waker,
                remote_wake_tx,
                remote_wakes_pending: AtomicBool::new(false),
            }),
        };
        (shared, remote_wake_rx)
    }

    pub(crate) fn thread_id(&self) -> ThreadId {
        self.inner.thread_id
    }

    pub(crate) fn drain_remote_wakes(&self, remote_wakes: &channel::Receiver<RemoteWake>) {
        if !self.inner.remote_wakes_pending.swap(false, atomic::Ordering::Acquire) {
            return;
        }
        loop {
            match remote_wakes.try_recv() {
                Ok(notification) => notification.schedule_local(self.inner.thread_id),
                Err(error) => {
                    debug_assert!(
                        error.is_empty() || error.is_closed(),
                        "remote wake channel reports only empty or closed: {error}"
                    );
                    return;
                }
            }
        }
    }

    pub(crate) fn close_remote_wakes(&self, remote_wakes: &channel::Receiver<RemoteWake>) {
        _ = remote_wakes.close();
        loop {
            match remote_wakes.try_recv() {
                Ok(notification) => drop(notification),
                Err(error) => {
                    debug_assert!(
                        error.is_empty() || error.is_closed(),
                        "closed remote wake channel reports only empty or closed: {error}"
                    );
                    // #[gamma::skip(loop.break_to_continue, loop.delete_break, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                    break;
                }
            }
        }
        self.inner.remote_wakes_pending.store(false, atomic::Ordering::Release);
    }

    pub(crate) fn has_remote_wakes(&self) -> bool {
        self.inner.remote_wakes_pending.load(atomic::Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub(crate) struct WakeSignal {
    task_ptr: TaskPtr,
    shared: WakeShared,
    schedule_state: AtomicU8,
    _requires_pin: PhantomPinned,
}

impl WakeSignal {
    pub(crate) fn new(shared: WakeShared, task_ptr: TaskPtr) -> Self {
        Self {
            task_ptr,
            shared,
            schedule_state: AtomicU8::new(TASK_ACTIVE),
            _requires_pin: PhantomPinned,
        }
    }

    pub(crate) fn retire(&self) {
        let previous = self.schedule_state.swap(0, atomic::Ordering::AcqRel);
        // #[gamma::skip(cond.always_true, reason = "remove_wake retains all nonmatching entries; when no local notification exists there is nothing to remove.")]
        if previous == TASK_ACTIVE_LOCAL {
            executor_context::remove_wake(self.shared.thread_id(), self.task_ptr);
        }
    }

    pub(crate) fn clear_queued_notification(&self) {
        self.clear_notification();
    }

    /// # Safety
    ///
    /// The signal and its enclosing task must remain pinned for the returned borrow.
    pub(crate) unsafe fn waker_ref(self: Pin<&Self>) -> impl Deref<Target = Waker> + '_ {
        let signal_ptr: *const Self = self.get_ref();
        BorrowedWakerRef {
            // SAFETY: The returned wrapper suppresses drop and is lifetime-bound to the signal.
            waker: ManuallyDrop::new(unsafe { Waker::from_raw(RawWaker::new(signal_ptr.cast(), &BORROWED_WAKER_VTABLE)) }),
            _lifetime: PhantomData,
        }
    }

    #[cfg(feature = "inspection")]
    pub(crate) fn has_queued_notification(&self) -> bool {
        // Acquire observes the scheduler's released state transitions.
        // This is a racing readiness hint, not access to queued task storage.
        matches!(
            self.schedule_state.load(atomic::Ordering::Acquire),
            TASK_ACTIVE_LOCAL | TASK_ACTIVE_REMOTE
        )
    }

    fn wake(&self) {
        let remote = !executor_context::is_current(self.shared.thread_id());
        if !self.begin_notification(remote) {
            return;
        }
        if remote {
            self.schedule_remote();
        } else {
            self.schedule_local();
        }
    }

    fn schedule_remote(&self) {
        let notification = RemoteWake::new(self);
        if let Err(error) = self.shared.inner.remote_wake_tx.try_send(notification) {
            self.clear_notification();
            drop(error.into_inner());
            return;
        }
        if !self.shared.inner.remote_wakes_pending.swap(true, atomic::Ordering::Release) {
            self.shared.inner.parent_waker.wake_by_ref();
        }
    }

    fn schedule_local(&self) {
        // SAFETY: The signal is embedded in the live task that owns this call.
        let task_handle = unsafe { self.task_ptr.retain() };
        if executor_context::enqueue(self.shared.thread_id(), task_handle).is_ok() {
            if !executor_context::is_executing(self.shared.thread_id()) {
                self.shared.inner.parent_waker.wake_by_ref();
            }

            return;
        }
        self.clear_notification();
        self.shared.inner.parent_waker.wake_by_ref();
    }

    fn begin_notification(&self, remote: bool) -> bool {
        let notification_state = if remote { TASK_ACTIVE_REMOTE } else { TASK_ACTIVE_LOCAL };
        matches!(
            self.schedule_state
                .try_update(atomic::Ordering::AcqRel, atomic::Ordering::Acquire, |state| match state {
                    TASK_ACTIVE => Some(notification_state),
                    TASK_ACTIVE_LOCAL | TASK_ACTIVE_REMOTE => Some(state),
                    _ => None,
                }),
            Ok(TASK_ACTIVE)
        )
    }

    pub(crate) fn promote_remote_notification(&self) -> bool {
        self.schedule_state
            .try_update(atomic::Ordering::AcqRel, atomic::Ordering::Acquire, |state| {
                (state == TASK_ACTIVE_REMOTE).then_some(TASK_ACTIVE_LOCAL)
            })
            .is_ok()
    }

    fn clear_notification(&self) {
        self.schedule_state.fetch_and(TASK_ACTIVE, atomic::Ordering::AcqRel);
    }

    fn add_owned_waker(&self) {
        // SAFETY: The borrowed signal is embedded in a live task. The increment transfers
        // independent ownership to the cloned waker.
        unsafe { self.task_ptr.increment_strong() };
    }
}

impl Drop for WakeSignal {
    fn drop(&mut self) {
        self.retire();
    }
}

pub(crate) struct RemoteWake(RemoteTaskHandle);

impl RemoteWake {
    fn new(signal: &WakeSignal) -> Self {
        // SAFETY: The live signal's enclosing task owns the allocation during notification setup.
        Self(unsafe { signal.task_ptr.retain_remote() })
    }

    fn schedule_local(self, owner: ThreadId) {
        // SAFETY: The receiving executor owns this thread, and the notification retains the task.
        if unsafe { self.0.as_task() }.promote_remote_notification() {
            // SAFETY: Remote wakes are drained only by the registered executor owner.
            let task_handle = unsafe { self.0.into_owner() };
            executor_context::enqueue(owner, task_handle).expect("remote wakes are drained by their registered executor owner");
        }
    }
}

impl std::fmt::Debug for RemoteWake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteWake").finish_non_exhaustive()
    }
}

struct BorrowedWakerRef<'a> {
    waker: ManuallyDrop<Waker>,
    _lifetime: PhantomData<&'a WakeSignal>,
}

impl Deref for BorrowedWakerRef<'_> {
    type Target = Waker;

    fn deref(&self) -> &Self::Target {
        &self.waker
    }
}

static BORROWED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone, borrowed_consume, waker_wake_by_ref, borrowed_drop);
static OWNED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone, waker_wake, waker_wake_by_ref, waker_drop);

#[cfg_attr(coverage_nightly, coverage(off))]
fn borrowed_consume(_: *const ()) {
    unreachable!("a borrowed polling waker cannot be consumed");
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn borrowed_drop(_: *const ()) {
    unreachable!("a borrowed polling waker cannot be dropped");
}

#[cfg(test)]
#[test]
#[should_panic(expected = "a borrowed polling waker cannot be consumed")]
fn nov4_borrowed_polling_waker_cannot_be_consumed() {
    borrowed_consume(std::ptr::null());
}

#[cfg(test)]
#[test]
#[should_panic(expected = "a borrowed polling waker cannot be dropped")]
fn nov4_borrowed_polling_waker_cannot_be_dropped() {
    borrowed_drop(std::ptr::null());
}

fn waker_clone(ptr: *const ()) -> RawWaker {
    let signal = signal_from_raw(ptr);
    signal.add_owned_waker();
    RawWaker::new(ptr, &OWNED_WAKER_VTABLE)
}

fn waker_wake(ptr: *const ()) {
    // SAFETY: Consuming wake transfers this raw waker's ownership into the guard.
    let _owner = unsafe { signal_from_raw(ptr).task_ptr.into_remote() };
    signal_from_raw(ptr).wake();
}

fn waker_wake_by_ref(ptr: *const ()) {
    signal_from_raw(ptr).wake();
}

fn waker_drop(ptr: *const ()) {
    // SAFETY: Dropping transfers this raw waker's ownership into the guard.
    drop(unsafe { signal_from_raw(ptr).task_ptr.into_remote() });
}

fn signal_from_raw(ptr: *const ()) -> &'static WakeSignal {
    // SAFETY: Raw waker functions are called only with the pinned signal pointer used to create
    // the waker. Borrowed calls are scoped to task polling. Owned calls retain the enclosing task.
    unsafe { ptr.cast::<WakeSignal>().as_ref().expect("raw waker signal pointer is never null") }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;
    use std::thread;

    use plurality::MultiPool;

    use super::*;

    #[test]
    fn nov4_executor_shutdown_closes_the_retained_task_wakers_remote_sender() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        use std::task::Poll;

        #[derive(Debug)]
        struct Completion(Rc<Cell<usize>>);

        impl crate::TaskCompletion<()> for Completion {
            fn completed(self, (): ()) {}

            fn panicked(self, panic: crate::TaskPanicPayload, _: Option<crate::ShutdownCause>) {
                crate::discard_task_panic_payload(panic);
            }

            fn cancelled(self, _: crate::ShutdownCause) {
                self.0.set(self.0.get() + 1);
            }
        }

        let captured = Rc::new(RefCell::new(None));
        let saved = Rc::clone(&captured);
        let cancellations = Rc::new(Cell::new(0));
        let mut executor = crate::Executor::builder().build();
        executor.tasks().add(
            std::future::poll_fn(move |cx| {
                *saved.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending::<()>
            }),
            crate::TaskOptions::default(),
            Completion(Rc::clone(&cancellations)),
        );
        let _ = executor.execute_cycle();
        let waker = captured.borrow_mut().take().unwrap();
        // SAFETY: this is an owned polling-waker clone created by this executor's
        // WakeSignal vtable; retaining it keeps that pinned signal alive.
        let signal = unsafe { &*waker.data().cast::<WakeSignal>() };
        let shared = signal.shared.clone();
        assert!(!shared.inner.remote_wake_tx.is_closed());
        executor.shutdown().trigger(crate::ShutdownCause::default());
        assert!(matches!(executor.execute_cycle(), crate::CycleOutcome::Shutdown(_)));
        assert!(shared.inner.remote_wake_tx.is_closed());
        assert_eq!(cancellations.get(), 1);
        assert!(!executor.has_tasks());
        waker.wake_by_ref();
        assert!(!shared.has_remote_wakes());
    }

    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn gamma_survivor_remote_close_drains_ownership_resets_flag_and_rejects_later_wakes() {
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        let (shared, receiver) = WakeShared::new(Waker::noop().clone(), thread::current().id());
        assert!(!shared.has_remote_wakes());
        let signal = pin!(WakeSignal::new(shared.clone(), task.as_ptr()));
        assert!(signal.begin_notification(true));
        signal.schedule_remote();
        assert!(shared.has_remote_wakes());
        assert!(signal.promote_remote_notification());
        assert_eq!(signal.schedule_state.load(atomic::Ordering::Acquire), TASK_ACTIVE_LOCAL);
        assert!(!signal.promote_remote_notification());
        shared.close_remote_wakes(&receiver);
        assert!(!shared.has_remote_wakes());
        assert!(receiver.try_recv().unwrap_err().is_closed());
        signal.clear_notification();
        assert!(signal.begin_notification(true));
        signal.schedule_remote();
        assert!(!shared.has_remote_wakes());
        assert_eq!(signal.schedule_state.load(atomic::Ordering::Acquire), TASK_ACTIVE);
        signal.retire();
        assert!(!signal.begin_notification(false));
        assert!(!signal.promote_remote_notification());
        drop(task);
        assert!(pool.is_empty());
    }

    #[test]
    fn gamma_survivor_draining_a_retired_remote_notification_does_not_requeue_task() {
        let owner = thread::current().id();
        executor_context::register(owner);
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        let (shared, _receiver) = WakeShared::new(Waker::noop().clone(), owner);
        // SAFETY: the handle owns its initialized task on this registered owner thread.
        unsafe { task.as_task() }.initialize(WakeSignal::new(shared, task.as_ptr()));
        // SAFETY: the handle retains the task while this notification acquires its own owner.
        let remote = RemoteWake(unsafe { task.as_ptr().retain_remote() });
        // SAFETY: task is still owned by its original thread.
        unsafe { task.as_task() }.abort(crate::ShutdownCause::default());
        remote.schedule_local(owner);
        assert!(!executor_context::has_wakes(owner));
        drop(task);
        assert!(pool.is_empty());
        executor_context::unregister(owner);
    }

    #[test]
    fn gamma_survivor_local_wakes_only_notify_idle_owner_and_retirement_removes_queue_owner() {
        let owner = thread::current().id();
        executor_context::register(owner);
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
        let (shared, _receiver) = WakeShared::new(Waker::from(Arc::clone(&counter)), owner);
        let signal = pin!(WakeSignal::new(shared, task.as_ptr()));
        assert!(signal.begin_notification(false));
        signal.schedule_local();
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        signal.retire();
        assert!(!executor_context::has_wakes(owner));
        assert_eq!(signal.schedule_state.load(atomic::Ordering::Acquire), 0);

        let signal = pin!(WakeSignal::new(signal.shared.clone(), task.as_ptr()));
        let guard = executor_context::enter_cycle(owner);
        assert!(signal.begin_notification(false));
        signal.schedule_local();
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        signal.retire();
        drop(guard);
        {
            let queued = pin!(WakeSignal::new(signal.shared.clone(), task.as_ptr()));
            assert!(queued.begin_notification(false));
            queued.schedule_local();
            assert!(executor_context::has_wakes(owner));
        }
        assert!(!executor_context::has_wakes(owner));
        executor_context::unregister(owner);
        drop(task);
        assert!(pool.is_empty());
    }

    #[test]
    fn retirement_removes_a_local_notification() {
        let _executor = crate::Executor::builder().build();
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        let (shared, _remote_wakes) = WakeShared::new(Waker::noop().clone(), thread::current().id());
        let signal = pin!(WakeSignal::new(shared, task.as_ptr()));

        assert!(signal.begin_notification(false));
        signal.schedule_local();
        assert!(executor_context::has_wakes(thread::current().id()));
        signal.retire();

        assert!(!executor_context::has_wakes(thread::current().id()));
        drop(task);
        assert!(pool.is_empty());
    }

    #[test]
    fn local_schedule_after_owner_loss_notifies_the_parent() {
        let pool = MultiPool::new();
        let foreign_owner = thread::spawn(|| thread::current().id()).join().unwrap();
        let parent = Arc::new(CountingWake(AtomicUsize::new(0)));
        let task = crate::allocate_test_task_in(&pool);
        let (shared, _remote_wakes) = WakeShared::new(Waker::from(Arc::clone(&parent)), foreign_owner);
        let signal = pin!(WakeSignal::new(shared, task.as_ptr()));

        assert!(signal.begin_notification(false));
        signal.schedule_local();

        assert_eq!(parent.0.load(Ordering::Relaxed), 1);
        assert_eq!(signal.schedule_state.load(atomic::Ordering::Acquire), TASK_ACTIVE);
        drop(task);
        assert!(pool.is_empty());
    }

    #[test]
    fn closed_remote_channel_clears_the_notification() {
        let pool = MultiPool::new();
        let foreign_owner = thread::spawn(|| thread::current().id()).join().unwrap();
        let task = crate::allocate_test_task_in(&pool);
        let (shared, remote_wakes) = WakeShared::new(Waker::noop().clone(), foreign_owner);
        drop(remote_wakes);
        let signal = pin!(WakeSignal::new(shared, task.as_ptr()));

        assert!(signal.begin_notification(true));
        signal.schedule_remote();

        assert_eq!(signal.schedule_state.load(atomic::Ordering::Acquire), TASK_ACTIVE);
        drop(task);
        assert!(pool.is_empty());
    }

    #[test]
    fn remote_wake_has_non_exhaustive_diagnostics() {
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        // SAFETY: `task` keeps the initialized template alive.
        let remote = RemoteWake(unsafe { task.as_ptr().retain_remote() });

        assert_eq!(format!("{remote:?}"), "RemoteWake { .. }");

        drop((remote, task));
        assert!(pool.is_empty());
    }

    #[test]
    fn duplicate_remote_wake_publishes_prior_writes() {
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        let (shared, remote_wakes) = WakeShared::new(Waker::noop().clone(), thread::current().id());
        let signal = pin!(WakeSignal::new(shared, task.as_ptr()));
        // SAFETY: The clone acquires independent task ownership before the borrow ends.
        let waker = unsafe { signal.as_ref().waker_ref() }.clone();
        let published = Arc::new(AtomicBool::new(false));
        let duplicate_complete = Arc::new(AtomicBool::new(false));

        thread::scope(|scope| {
            let producer_published = Arc::clone(&published);
            let producer_complete = Arc::clone(&duplicate_complete);
            scope.spawn(move || {
                waker.wake_by_ref();
                producer_published.store(true, atomic::Ordering::Relaxed);
                waker.wake_by_ref();
                producer_complete.store(true, atomic::Ordering::Relaxed);
            });

            while !duplicate_complete.load(atomic::Ordering::Relaxed) {
                thread::yield_now();
            }
            signal.clear_notification();
            assert!(published.load(atomic::Ordering::Relaxed));
        });

        drop(remote_wakes);
        drop(task);
        assert!(pool.is_empty());
    }

    #[test]
    fn dropping_an_owned_waker_releases_its_task_reference() {
        let pool = MultiPool::new();
        let task = crate::allocate_test_task_in(&pool);
        let (shared, _remote_wakes) = WakeShared::new(Waker::noop().clone(), thread::current().id());
        let signal = pin!(WakeSignal::new(shared, task.as_ptr()));
        // SAFETY: The clone acquires independent task ownership before the borrow ends.
        let waker = unsafe { signal.as_ref().waker_ref() }.clone();

        drop(task);
        assert_eq!(pool.len(), 1);
        drop(waker);

        assert!(pool.is_empty());
    }
}
