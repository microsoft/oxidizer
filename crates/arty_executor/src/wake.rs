// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
use std::collections::VecDeque;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU8};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::thread::ThreadId;

use performables::sync::channel;

use crate::{TaskHandle, TaskPtr, executor_context};

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

    #[cfg(test)]
    pub(crate) fn new_for_test(parent_waker: Waker, task_handle: &TaskHandle) -> Self {
        let (shared, _remote_wakes) = WakeShared::new(parent_waker, std::thread::current().id());
        Self::new(shared, task_handle.as_ptr())
    }

    pub(crate) fn retire(&self) {
        let previous = self.schedule_state.swap(0, atomic::Ordering::AcqRel);
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

    #[cfg(test)]
    pub(crate) fn has_queued_notification(&self) -> bool {
        matches!(
            self.schedule_state.load(atomic::Ordering::Acquire),
            TASK_ACTIVE_LOCAL | TASK_ACTIVE_REMOTE
        )
    }

    #[cfg(test)]
    pub(crate) fn append_test_wakes(&self, target: &mut VecDeque<TaskHandle>) {
        executor_context::append_wakes(self.shared.thread_id(), target);
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
        self.shared.inner.remote_wakes_pending.store(true, atomic::Ordering::Release);
        self.shared.inner.parent_waker.wake_by_ref();
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
        self.schedule_state
            .try_update(atomic::Ordering::AcqRel, atomic::Ordering::Acquire, |state| {
                (state == TASK_ACTIVE).then_some(notification_state)
            })
            .is_ok()
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
        // SAFETY: The borrowed signal is embedded in a live task; the increment transfers
        // independent ownership to the cloned waker.
        unsafe { self.task_ptr.increment_strong() };
    }
}

impl Drop for WakeSignal {
    fn drop(&mut self) {
        self.retire();
    }
}

pub(crate) struct RemoteWake(TaskHandle);

impl RemoteWake {
    fn new(signal: &WakeSignal) -> Self {
        // SAFETY: The live signal's enclosing task owns the allocation during notification setup.
        Self(unsafe { signal.task_ptr.retain() })
    }

    fn schedule_local(self, owner: ThreadId) {
        // SAFETY: The receiving executor owns this thread, and the notification retains the task.
        if unsafe { self.0.as_task() }.promote_remote_notification() {
            executor_context::enqueue(owner, self.0).expect("remote wakes are drained by their registered executor owner");
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

fn waker_clone(ptr: *const ()) -> RawWaker {
    let signal = signal_from_raw(ptr);
    signal.add_owned_waker();
    RawWaker::new(ptr, &OWNED_WAKER_VTABLE)
}

fn waker_wake(ptr: *const ()) {
    // SAFETY: Consuming wake transfers this raw waker's ownership into the guard.
    let _owner = unsafe { signal_from_raw(ptr).task_ptr.into_handle() };
    signal_from_raw(ptr).wake();
}

fn waker_wake_by_ref(ptr: *const ()) {
    signal_from_raw(ptr).wake();
}

fn waker_drop(ptr: *const ()) {
    // SAFETY: Dropping transfers this raw waker's ownership into the guard.
    drop(unsafe { signal_from_raw(ptr).task_ptr.into_handle() });
}

fn signal_from_raw(ptr: *const ()) -> &'static WakeSignal {
    // SAFETY: Raw waker functions are called only with the pinned signal pointer used to create
    // the waker. Borrowed calls are scoped to task polling; owned calls retain the enclosing task.
    unsafe { ptr.cast::<WakeSignal>().as_ref().expect("raw waker signal pointer is never null") }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::pin::pin;
    use std::sync::atomic::Ordering;
    use std::thread;

    use events_once::LocalEventPool;
    use plurality::MultiPool;

    use super::*;
    use crate::testing::TestWaker;

    #[test]
    fn borrowed_and_owned_wakes_share_inline_state() {
        let _executor = crate::Executor::builder().build();
        let parent = Arc::new(TestWaker::new());
        let task_handle = crate::testing::new_test_task_handle();
        let signal = pin!(WakeSignal::new_for_test(Arc::clone(&parent).into(), &task_handle));

        // SAFETY: The signal remains pinned for the borrow and clone.
        let borrowed = unsafe { signal.as_ref().waker_ref() };
        let owned = borrowed.clone();
        borrowed.wake_by_ref();
        owned.wake_by_ref();

        let mut queued = VecDeque::new();
        signal.append_test_wakes(&mut queued);
        assert_eq!(queued.len(), 1);
        assert!(parent.awakened.load(Ordering::Relaxed));
        drop(owned);
    }

    #[test]
    fn retirement_makes_retained_waker_inert() {
        let _executor = crate::Executor::builder().build();
        let task_handle = crate::testing::new_test_task_handle();
        let signal = pin!(WakeSignal::new_for_test(Waker::noop().clone(), &task_handle));
        // SAFETY: The clone acquires independent task ownership before the borrow ends.
        let waker = unsafe { signal.as_ref().waker_ref() }.clone();

        waker.wake_by_ref();
        assert!(executor_context::has_wakes(signal.shared.thread_id()));
        signal.retire();
        assert!(!executor_context::has_wakes(signal.shared.thread_id()));
        waker.wake_by_ref();
        assert!(!executor_context::has_wakes(signal.shared.thread_id()));
    }

    #[test]
    fn dropping_a_locally_notified_signal_removes_its_queued_wake() {
        let _executor = crate::Executor::builder().build();
        let task_handle = crate::testing::new_test_task_handle();
        {
            let signal = pin!(WakeSignal::new_for_test(Waker::noop().clone(), &task_handle));
            // SAFETY: The signal remains pinned until its queued notification is retired on drop.
            unsafe { signal.as_ref().waker_ref() }.wake_by_ref();
            assert!(executor_context::has_wakes(signal.shared.thread_id()));
        }

        assert!(!executor_context::has_wakes(std::thread::current().id()));
    }

    #[test]
    fn local_schedule_after_owner_loss_notifies_parent() {
        let foreign_owner = thread::spawn(|| thread::current().id()).join().unwrap();
        let parent = Arc::new(TestWaker::new());
        let task_handle = crate::testing::new_test_task_handle();
        let (shared, _remote_wakes) = WakeShared::new(Arc::clone(&parent).into(), foreign_owner);
        let signal = pin!(WakeSignal::new(shared, task_handle.as_ptr()));

        assert!(signal.begin_notification(false));
        signal.schedule_local();

        assert!(parent.awakened.load(Ordering::Relaxed));
        assert!(!signal.has_queued_notification());
    }

    #[test]
    fn closed_remote_channel_clears_notification() {
        let foreign_owner = thread::spawn(|| thread::current().id()).join().unwrap();
        let task_handle = crate::testing::new_test_task_handle();
        let (shared, remote_wakes) = WakeShared::new(Waker::noop().clone(), foreign_owner);
        drop(remote_wakes);
        let signal = pin!(WakeSignal::new(shared, task_handle.as_ptr()));

        // SAFETY: The signal remains pinned for the borrowed wake.
        unsafe { signal.as_ref().waker_ref() }.wake_by_ref();

        assert!(!signal.has_queued_notification());
    }

    #[test]
    fn remote_wake_has_non_exhaustive_diagnostics() {
        let task_handle = crate::testing::new_test_task_handle();
        let remote = RemoteWake(task_handle);

        assert_eq!(format!("{remote:?}"), "RemoteWake { .. }");
    }

    #[test]
    fn remote_wake_pending_state_is_observable() {
        let (shared, _remote_wakes) = WakeShared::new(Waker::noop().clone(), std::thread::current().id());
        assert!(!shared.has_remote_wakes());

        shared.inner.remote_wakes_pending.store(true, atomic::Ordering::Release);

        assert!(shared.has_remote_wakes());
    }

    #[test]
    fn only_remote_notifications_can_be_promoted() {
        let task_handle = crate::testing::new_test_task_handle();
        let signal = pin!(WakeSignal::new_for_test(Waker::noop().clone(), &task_handle));

        assert!(!signal.promote_remote_notification());
        assert!(signal.begin_notification(true));
        assert!(signal.promote_remote_notification());
        assert!(!signal.promote_remote_notification());
        signal.retire();
        assert!(!signal.promote_remote_notification());
    }

    #[test]
    fn dropping_an_owned_waker_releases_its_task_reference() {
        let pool = MultiPool::new();
        let (sender, _receiver) = LocalEventPool::<()>::new().rent();
        let task_handle = TaskHandle::new(pool.alloc_arc_pin(crate::Task::new(pending(), sender)));
        {
            let signal = pin!(WakeSignal::new_for_test(Waker::noop().clone(), &task_handle));
            // SAFETY: The clone acquires independent task ownership before the borrow ends.
            let waker = unsafe { signal.as_ref().waker_ref() }.clone();

            drop(waker);
        }
        drop(task_handle);

        assert!(pool.is_empty());
    }
}
