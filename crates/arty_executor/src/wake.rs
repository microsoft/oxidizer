// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU8};
use std::task::{RawWaker, RawWakerVTable, Waker};

use performables::sync::channel;

use crate::{TaskPointer, TaskRef, executor_context};

const TASK_ACTIVE: u8 = 1 << 0;
const TASK_LOCAL_NOTIFIED: u8 = 1 << 1;
const TASK_REMOTE_NOTIFIED: u8 = 1 << 2;
const TASK_NOTIFIED: u8 = TASK_LOCAL_NOTIFIED | TASK_REMOTE_NOTIFIED;

#[derive(Debug)]
pub(crate) struct WakeShared {
    parent_waker: Waker,
    remote_wake_tx: channel::Sender<RemoteWake>,
    remote_wake_rx: channel::Receiver<RemoteWake>,
    remote_wakes_pending: AtomicBool,
}

impl WakeShared {
    pub(crate) fn new(parent_waker: Waker) -> Self {
        let (remote_wake_tx, remote_wake_rx) = channel::unbounded();
        Self {
            parent_waker,
            remote_wake_tx,
            remote_wake_rx,
            remote_wakes_pending: AtomicBool::new(false),
        }
    }

    fn is_current(&self) -> bool {
        executor_context::is_current(self)
    }

    pub(crate) fn enter_cycle(&self) -> executor_context::CycleGuard {
        executor_context::enter_cycle(self)
    }

    fn is_executing(&self) -> bool {
        executor_context::is_executing(self)
    }

    fn enqueue_local(&self, task_ref: TaskRef) -> Result<(), TaskRef> {
        executor_context::enqueue(self, task_ref)
    }

    pub(crate) fn append_local_wakes(&self, target: &mut VecDeque<TaskRef>) {
        executor_context::append_wakes(self, target);
    }

    pub(crate) fn has_local_wakes(&self) -> bool {
        executor_context::has_wakes(self)
    }

    fn remove_local_notification(&self, task_ref: TaskPointer) {
        executor_context::remove_wake(self, task_ref);
    }

    pub(crate) fn clear_local_wakes(&self) {
        executor_context::clear_wakes(self);
    }

    pub(crate) fn drain_remote_wakes(&self) {
        if !self.remote_wakes_pending.swap(false, atomic::Ordering::Acquire) {
            return;
        }
        while !self.remote_wake_rx.is_empty() {
            match self.remote_wake_rx.try_recv() {
                Ok(notification) => notification.schedule_local(),
                Err(error) if error.is_empty() || error.is_closed() => return,
                Err(error) => unreachable!("remote wake channel reports only empty or closed: {error}"),
            }
        }
    }

    pub(crate) fn has_remote_wakes(&self) -> bool {
        self.remote_wakes_pending.load(atomic::Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub(crate) struct WakeSignal {
    task_ref: TaskPointer,
    shared: Arc<WakeShared>,
    schedule_state: AtomicU8,
    _requires_pin: PhantomPinned,
    #[cfg(test)]
    _test_task_owner: Option<TaskRef>,
}

impl WakeSignal {
    pub(crate) fn new(shared: Arc<WakeShared>, task_ref: TaskPointer) -> Self {
        Self {
            task_ref,
            shared,
            schedule_state: AtomicU8::new(TASK_ACTIVE),
            _requires_pin: PhantomPinned,
            #[cfg(test)]
            _test_task_owner: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(parent_waker: Waker, task_ref: TaskRef) -> Self {
        let shared = Arc::new(WakeShared::new(parent_waker));
        let task_pointer = task_ref.pointer();
        let mut signal = Self::new(shared, task_pointer);
        signal._test_task_owner = Some(task_ref);
        signal
    }

    #[cfg(test)]
    pub(crate) fn fake() -> Self {
        // SAFETY: Tests use the allocation only as stable task identity.
        let fake_task_ref = unsafe { TaskRef::fake() };
        Self::new_for_test(Waker::noop().clone(), fake_task_ref)
    }

    pub(crate) fn retire(&self) {
        let previous = self.schedule_state.swap(0, atomic::Ordering::AcqRel);
        if previous & TASK_LOCAL_NOTIFIED != 0 {
            self.shared.remove_local_notification(self.task_ref);
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
    pub(crate) unsafe fn waker(self: Pin<&Self>) -> Waker {
        // SAFETY: The signal remains pinned until the owned clone has acquired task ownership.
        unsafe { self.waker_ref() }.clone()
    }

    #[cfg(test)]
    pub(crate) fn has_queued_notification(&self) -> bool {
        self.schedule_state.load(atomic::Ordering::Acquire) & TASK_NOTIFIED != 0
    }

    #[cfg(test)]
    pub(crate) fn append_test_wakes(&self, target: &mut VecDeque<TaskRef>) {
        self.shared.append_local_wakes(target);
    }

    fn wake(&self) {
        let remote = !self.shared.is_current();
        if !self.begin_notification(remote) {
            return;
        }
        if remote {
            self.schedule_remote();
        } else {
            self.schedule_local(true);
        }
    }

    fn schedule_remote(&self) {
        let notification = RemoteWake::new(self);
        if let Err(error) = self.shared.remote_wake_tx.try_send(notification) {
            self.clear_notification();
            drop(error.into_inner());
            return;
        }
        self.shared.remote_wakes_pending.store(true, atomic::Ordering::Release);
        self.shared.parent_waker.wake_by_ref();
    }

    fn schedule_local(&self, wake_owner: bool) {
        // SAFETY: The signal is embedded in the live task that owns this call.
        let task_ref = unsafe { self.task_ref.retain() };
        if self.shared.enqueue_local(task_ref).is_ok() {
            if wake_owner && !self.shared.is_executing() {
                self.shared.parent_waker.wake_by_ref();
            }

            return;
        }
        self.clear_notification();
        if wake_owner {
            self.shared.parent_waker.wake_by_ref();
        }
    }

    fn begin_notification(&self, remote: bool) -> bool {
        let notification = if remote { TASK_REMOTE_NOTIFIED } else { TASK_LOCAL_NOTIFIED };
        let mut state = self.schedule_state.load(atomic::Ordering::Acquire);
        loop {
            if state & TASK_ACTIVE == 0 || state & TASK_NOTIFIED != 0 {
                return false;
            }
            match self.schedule_state.compare_exchange_weak(
                state,
                state | notification,
                atomic::Ordering::AcqRel,
                atomic::Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => state = observed,
            }
        }
    }

    fn promote_remote_notification(&self) -> bool {
        let mut state = self.schedule_state.load(atomic::Ordering::Acquire);
        loop {
            if state & TASK_ACTIVE == 0 || state & TASK_REMOTE_NOTIFIED == 0 {
                return false;
            }
            let local = (state & !TASK_REMOTE_NOTIFIED) | TASK_LOCAL_NOTIFIED;
            match self
                .schedule_state
                .compare_exchange_weak(state, local, atomic::Ordering::AcqRel, atomic::Ordering::Acquire)
            {
                Ok(_) => return true,
                Err(observed) => state = observed,
            }
        }
    }

    fn clear_notification(&self) {
        self.schedule_state.fetch_and(TASK_ACTIVE, atomic::Ordering::AcqRel);
    }

    fn add_owned_waker(&self) {
        // SAFETY: The borrowed signal is embedded in a live task; the increment transfers
        // independent ownership to the cloned waker.
        unsafe { self.task_ref.increment_strong() };
    }
}

impl Drop for WakeSignal {
    fn drop(&mut self) {
        self.retire();
    }
}

struct RemoteWake {
    signal: NonNull<WakeSignal>,
    task_ref: TaskPointer,
}

// SAFETY: The task strong reference keeps the signal pinned and alive. Scheduling state is atomic.
unsafe impl Send for RemoteWake {}

impl RemoteWake {
    fn new(signal: &WakeSignal) -> Self {
        // SAFETY: The live signal's enclosing task owns the allocation during notification setup.
        unsafe { signal.task_ref.increment_strong() };
        Self {
            signal: NonNull::from(signal),
            task_ref: signal.task_ref,
        }
    }

    fn schedule_local(self) {
        // SAFETY: This notification owns a strong task reference containing the pinned signal.
        let signal = unsafe { self.signal.as_ref() };
        if signal.promote_remote_notification() {
            signal.schedule_local(false);
        }
    }
}

impl Drop for RemoteWake {
    fn drop(&mut self) {
        // SAFETY: `RemoteWake::new` created exactly one raw strong reference for this owner.
        unsafe { self.task_ref.release_strong() };
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

struct RawWakerOwner(TaskPointer);

impl RawWakerOwner {
    unsafe fn from_raw(ptr: *const ()) -> Self {
        Self(signal_from_raw(ptr).task_ref)
    }
}

impl Drop for RawWakerOwner {
    fn drop(&mut self) {
        // SAFETY: The raw waker transferred exactly one task strong reference into this guard.
        unsafe { self.0.release_strong() };
    }
}

static BORROWED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(borrowed_clone, borrowed_consume, borrowed_wake_by_ref, borrowed_drop);
static OWNED_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(waker_clone, waker_wake, waker_wake_by_ref, waker_drop);

fn borrowed_clone(ptr: *const ()) -> RawWaker {
    let signal = signal_from_raw(ptr);
    signal.add_owned_waker();
    RawWaker::new(ptr, &OWNED_WAKER_VTABLE)
}

fn borrowed_wake_by_ref(ptr: *const ()) {
    signal_from_raw(ptr).wake();
}

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
    let _owner = unsafe { RawWakerOwner::from_raw(ptr) };
    signal_from_raw(ptr).wake();
}

fn waker_wake_by_ref(ptr: *const ()) {
    signal_from_raw(ptr).wake();
}

fn waker_drop(ptr: *const ()) {
    // SAFETY: Dropping transfers this raw waker's ownership into the guard.
    drop(unsafe { RawWakerOwner::from_raw(ptr) });
}

fn signal_from_raw(ptr: *const ()) -> &'static WakeSignal {
    // SAFETY: Raw waker functions are called only with the pinned signal pointer used to create
    // the waker. Borrowed calls are scoped to task polling; owned calls retain the enclosing task.
    unsafe { ptr.cast::<WakeSignal>().as_ref().expect("raw waker signal pointer is never null") }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::testing::TestWaker;

    #[test]
    fn borrowed_and_owned_wakes_share_inline_state() {
        let parent = Arc::new(TestWaker::new());
        // SAFETY: Tests use the allocation only as stable task identity.
        let task_ref = unsafe { TaskRef::fake() };
        let signal = pin!(WakeSignal::new_for_test(Arc::clone(&parent).into(), task_ref));

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
        // SAFETY: Tests use the allocation only as stable task identity.
        let task_ref = unsafe { TaskRef::fake() };
        let signal = pin!(WakeSignal::new_for_test(Waker::noop().clone(), task_ref));
        // SAFETY: The clone acquires independent task ownership before the borrow ends.
        let waker = unsafe { signal.as_ref().waker() };

        waker.wake_by_ref();
        assert!(signal.shared.has_local_wakes());
        signal.retire();
        assert!(!signal.shared.has_local_wakes());
        waker.wake_by_ref();
        assert!(!signal.shared.has_local_wakes());
    }
}
