// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Waker;

/// Stack-allocated state shared between the async caller and the worker thread.
///
/// # Cancellation Safety
///
/// The [`Drop`] implementation spin-loops until the `ready` flag is set. This
/// guarantees the worker thread can always safely write its result to the
/// stack-allocated slots, even if the future is cancelled. However, it also
/// means that dropping a `StackState` whose work item was **never dispatched**
/// (or whose worker will never call [`complete`](Self::complete)) will block
/// the dropping thread indefinitely. Callers must ensure the work item is
/// submitted to the [`Thunker`](crate::Thunker) before the `StackState` can
/// be dropped.
pub struct StackState<R, T> {
    ready: AtomicBool,
    panicked: AtomicBool,
    result: UnsafeCell<Option<R>>,
    waker: Mutex<Option<Waker>>,
    task: UnsafeCell<Option<T>>,
}

impl<R, T> StackState<R, T> {
    /// Creates a new empty `StackState`.
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            panicked: AtomicBool::new(false),
            result: UnsafeCell::new(None),
            waker: Mutex::new(None),
            task: UnsafeCell::new(None),
        }
    }

    /// Stores the task arguments into the state.
    ///
    /// # Safety
    ///
    /// Must not be called concurrently with [`take_task`](Self::take_task).
    pub unsafe fn set_task(&self, task: T) {
        // SAFETY: Caller guarantees no concurrent access to the task slot.
        unsafe { *self.task.get() = Some(task) };
    }

    /// Takes the task arguments out of the state, returning `None` if already taken.
    ///
    /// # Safety
    ///
    /// Must not be called concurrently with [`set_task`](Self::set_task).
    pub unsafe fn take_task(&self) -> Option<T> {
        // SAFETY: Caller guarantees no concurrent access to the task slot.
        unsafe { (*self.task.get()).take() }
    }

    /// Writes the computed result and signals readiness.
    ///
    /// # Safety
    ///
    /// Must be called exactly once by the worker thread after computing the result.
    pub unsafe fn complete(&self, result: R) {
        // SAFETY: Caller guarantees exclusive access to the result slot at this point.
        unsafe { *self.result.get() = Some(result) };
        self.ready.store(true, Ordering::Release);
    }

    /// Takes the result out of the state, returning `None` if not yet written.
    ///
    /// # Safety
    ///
    /// Must only be called after [`is_ready`](Self::is_ready) returns `true`.
    pub unsafe fn take_result(&self) -> Option<R> {
        // SAFETY: Caller guarantees the result has been written and no concurrent access.
        unsafe { (*self.result.get()).take() }
    }

    /// Returns `true` if the worker has signaled completion.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Marks the state as panicked and signals readiness.
    ///
    /// This unblocks `Drop` and causes `ThunkFuture::poll` to re-raise the panic.
    pub fn mark_panicked(&self) {
        self.panicked.store(true, Ordering::Relaxed);
        self.ready.store(true, Ordering::Release);
    }

    /// Returns `true` if the worker panicked.
    pub fn has_panicked(&self) -> bool {
        self.panicked.load(Ordering::Relaxed)
    }

    /// Stores a waker to be notified when the result is ready.
    ///
    /// # Panics
    ///
    /// Panics if the internal waker mutex is poisoned.
    pub fn set_waker(&self, waker: Waker) {
        let mut guard = self.waker.lock().expect("waker mutex is not poisoned");
        *guard = Some(waker);
    }

    /// Takes and wakes the stored waker, if present.
    ///
    /// # Panics
    ///
    /// Panics if the internal waker mutex is poisoned.
    pub fn wake(&self) {
        let waker = self.waker.lock().expect("waker mutex is not poisoned").take();
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Returns a raw const pointer to this `StackState`.
    pub fn as_ptr(&self) -> *const Self {
        self
    }

    /// Returns a raw mutable pointer to this `StackState`.
    pub fn as_mut_ptr(&self) -> *mut Self {
        std::ptr::from_ref(self).cast_mut()
    }
}

impl<R, T> Default for StackState<R, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R, T> core::fmt::Debug for StackState<R, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StackState")
            .field("ready", &self.ready.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<R, T> Drop for StackState<R, T> {
    fn drop(&mut self) {
        // Cancellation guard: prevent use-after-free if the future is dropped
        // before the worker finishes writing to our stack-allocated state.
        while !self.ready.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
    }
}

// SAFETY: StackState is designed for cross-thread sharing between an async
// poller and a worker thread. Access to UnsafeCell fields is synchronized
// by the `ready` atomic flag and the protocol enforced by the unsafe methods.
unsafe impl<R: Send, T: Send> Sync for StackState<R, T> {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::task::{RawWaker, RawWakerVTable};

    use super::*;

    fn noop_clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &NOOP_VTABLE)
    }
    fn noop(_: *const ()) {}
    static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);

    /// Creates a no-op waker for testing.
    fn noop_waker() -> Waker {
        // SAFETY: The vtable functions are sound no-ops.
        unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_VTABLE)) }
    }

    fn flag_clone(data: *const ()) -> RawWaker {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        let arc = unsafe { Arc::from_raw(data.cast::<AtomicBool>()) };
        let clone = Arc::clone(&arc);
        core::mem::forget(arc);
        RawWaker::new(Arc::into_raw(clone).cast(), &FLAG_VTABLE)
    }
    fn flag_wake(data: *const ()) {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        let arc = unsafe { Arc::from_raw(data.cast::<AtomicBool>()) };
        arc.store(true, Ordering::SeqCst);
    }
    fn flag_wake_by_ref(data: *const ()) {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        let arc = unsafe { Arc::from_raw(data.cast::<AtomicBool>()) };
        arc.store(true, Ordering::SeqCst);
        core::mem::forget(arc);
    }
    fn flag_drop(data: *const ()) {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        unsafe { drop(Arc::from_raw(data.cast::<AtomicBool>())) };
    }
    static FLAG_VTABLE: RawWakerVTable = RawWakerVTable::new(flag_clone, flag_wake, flag_wake_by_ref, flag_drop);

    #[test]
    fn new_is_not_ready() {
        let state = StackState::<u32, u32>::new();
        assert!(!state.is_ready());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
    }

    #[test]
    fn default_is_not_ready() {
        let state = StackState::<u32, u32>::default();
        assert!(!state.is_ready());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
    }

    #[test]
    fn set_and_take_task() {
        let state = StackState::<(), String>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.set_task(String::from("hello")) };
        // SAFETY: No concurrent access — single-threaded test.
        let task = unsafe { state.take_task() };
        assert_eq!(task.as_deref(), Some("hello"));
        // SAFETY: No concurrent access — single-threaded test.
        let task2 = unsafe { state.take_task() };
        assert!(task2.is_none());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
    }

    #[test]
    fn complete_and_take_result() {
        let state = StackState::<String, ()>::new();
        assert!(!state.is_ready());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(String::from("result")) };
        assert!(state.is_ready());
        // SAFETY: is_ready() returned true; no concurrent access.
        let val = unsafe { state.take_result() };
        assert_eq!(val.as_deref(), Some("result"));
        // SAFETY: No concurrent access — single-threaded test.
        let val2 = unsafe { state.take_result() };
        assert!(val2.is_none());
    }

    #[test]
    fn set_waker_and_wake() {
        let woken = Arc::new(AtomicBool::new(false));
        let woken2 = Arc::clone(&woken);

        let raw = RawWaker::new(Arc::into_raw(woken2).cast(), &FLAG_VTABLE);
        // SAFETY: The vtable functions correctly manage Arc refcounts.
        let waker = unsafe { Waker::from_raw(raw) };

        let state = StackState::<(), ()>::new();
        state.set_waker(waker);
        state.wake();
        assert!(woken.load(Ordering::SeqCst));
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
    }

    #[test]
    fn wake_without_waker_is_noop() {
        let state = StackState::<(), ()>::new();
        state.wake();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
    }

    #[test]
    fn as_ptr_and_as_mut_ptr() {
        let state = StackState::<u32, u32>::new();
        let p = state.as_ptr();
        let mp = state.as_mut_ptr();
        assert_eq!(p, mp.cast_const());
        assert_eq!(p, &raw const state);
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
    }

    #[test]
    fn debug_impl_not_ready() {
        let state = StackState::<u32, u32>::new();
        let debug = format!("{state:?}");
        assert!(debug.contains("StackState"));
        assert!(debug.contains("false"));
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
    }

    #[test]
    fn debug_impl_ready() {
        let state = StackState::<u32, u32>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(42) };
        let debug = format!("{state:?}");
        assert!(debug.contains("true"));
    }

    #[test]
    fn drop_blocks_until_ready() {
        let state = StackState::<u32, u32>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(99) };
        drop(state);
    }

    #[test]
    fn sync_trait_bounds() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<StackState<u32, u32>>();
    }

    #[test]
    fn set_waker_with_noop() {
        let state = StackState::<(), ()>::new();
        state.set_waker(noop_waker());
        state.wake();
        state.wake();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
    }

    #[test]
    fn complete_with_complex_type() {
        let state = StackState::<Vec<String>, ()>::new();
        let data = vec![String::from("a"), String::from("b")];
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(data) };
        assert!(state.is_ready());
        // SAFETY: is_ready() returned true; no concurrent access.
        let result = unsafe { state.take_result() }.unwrap();
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn task_with_complex_type() {
        let state = StackState::<(), Vec<u32>>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.set_task(vec![1, 2, 3]) };
        // SAFETY: No concurrent access — single-threaded test.
        let task = unsafe { state.take_task() }.unwrap();
        assert_eq!(task, vec![1, 2, 3]);
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
    }
}
