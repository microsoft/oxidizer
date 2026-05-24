// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use crate::StackState;

/// A future that resolves when the worker thread writes a result into [`StackState`].
pub struct ThunkFuture<'a, R, T> {
    state: &'a StackState<R, T>,
}

impl<'a, R, T> ThunkFuture<'a, R, T> {
    /// Creates a new `ThunkFuture` that will resolve when the given state becomes ready.
    pub fn new(state: &'a StackState<R, T>) -> Self {
        Self { state }
    }
}

impl<R, T> core::fmt::Debug for ThunkFuture<'_, R, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThunkFuture").finish_non_exhaustive()
    }
}

// ThunkFuture holds only a plain reference — no self-referential state — so it
// is safe to move after pinning.
impl<R, T> Unpin for ThunkFuture<'_, R, T> {}

impl<R, T> Future for ThunkFuture<'_, R, T> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.state.is_ready() {
            return Poll::Ready(take_or_resume(self.state));
        }

        // Register the waker before re-checking readiness to avoid lost wakeups.
        // `set_waker(&Waker)` clones internally only when the stored waker is
        // not equivalent (Waker::will_wake) to the one supplied — typical
        // re-polls by the same executor task pay zero allocations here.
        self.state.set_waker(cx.waker());

        // Re-check after waker registration: the worker may have completed
        // between our first is_ready() check and the set_waker() call above.
        if self.state.is_ready() {
            return Poll::Ready(take_or_resume(self.state));
        }

        Poll::Pending
    }
}

/// Helper invoked once the state is observed as `ready`. Either returns the
/// worker's result, or re-raises the original panic on the awaiter's task by
/// handing the captured payload to [`std::panic::resume_unwind`]. This
/// preserves the panic's downcastable type (e.g. `String`, `&'static str`,
/// or a user-defined payload), matching the behaviour callers would observe
/// from a synchronous call.
fn take_or_resume<R, T>(state: &StackState<R, T>) -> R {
    // SAFETY: `is_ready()` returned true; the worker has stored the outcome
    // and will not touch the slot again. We are the only consumer.
    let outcome = unsafe { state.take_outcome() }.expect("ThunkFuture polled after it returned Ready (broken Future contract)");
    match outcome {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

    fn noop_clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &NOOP_VTABLE)
    }
    fn noop(_: *const ()) {}
    static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);

    /// Creates a no-op waker for manual polling.
    fn noop_waker() -> Waker {
        // SAFETY: The vtable functions are sound no-ops.
        unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_VTABLE)) }
    }

    #[test]
    fn poll_ready_immediately() {
        let state = StackState::<u32, ()>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(42) };
        state.mark_worker_done();

        let mut future = ThunkFuture::new(&state);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, 42),
            Poll::Pending => panic!("expected Ready"),
        }
    }

    #[test]
    fn poll_pending_then_ready() {
        let state = StackState::<String, ()>::new();

        let mut future = ThunkFuture::new(&state);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(Pin::new(&mut future).poll(&mut cx).is_pending());

        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(String::from("done")) };
        state.mark_worker_done();

        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, "done"),
            Poll::Pending => panic!("expected Ready"),
        }
    }

    #[test]
    fn poll_ready_on_recheck_after_waker_registration() {
        let state = StackState::<i64, ()>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(-1) };
        state.mark_worker_done();

        let mut future = ThunkFuture::new(&state);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, -1),
            Poll::Pending => panic!("expected Ready"),
        }
    }

    /// Force the post-`set_waker` re-check path (line 48) to fire by using a
    /// custom waker whose `clone()` implementation completes the state. On
    /// first poll, `is_ready()` returns false; `set_waker` triggers
    /// `waker.clone()` (because the slot is empty); the clone completes the
    /// state; the re-check sees `is_ready() == true` and returns Ready.
    #[test]
    fn poll_recheck_observes_completion_during_set_waker() {
        use std::ptr::NonNull;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Shared {
            state: NonNull<StackState<i64, ()>>,
            armed: AtomicBool,
        }
        // SAFETY: only the test thread accesses this; impls needed only for
        // the static-vtable lifetime of the RawWaker data pointer.
        unsafe impl Send for Shared {}
        // SAFETY: see `Send` impl above; the test never shares `Shared`
        // across threads.
        unsafe impl Sync for Shared {}

        fn race_clone(p: *const ()) -> RawWaker {
            // SAFETY: `p` always points to a live `Shared` for the lifetime
            // of the test; the data is keyed off `armed` to fire exactly
            // once.
            let s = unsafe { &*p.cast::<Shared>() };
            if s.armed.swap(false, Ordering::SeqCst) {
                // SAFETY: `state` is a live `StackState` owned by the test
                // frame; no other thread is touching it.
                let st = unsafe { s.state.as_ref() };
                // SAFETY: see above; `complete` requires the worker-side
                // exclusive access we have here as the only producer.
                unsafe { st.complete(123) };
                st.mark_worker_done();
            }
            RawWaker::new(p, &RACE_VTABLE)
        }
        fn race_noop(_: *const ()) {}
        static RACE_VTABLE: RawWakerVTable = RawWakerVTable::new(race_clone, race_noop, race_noop, race_noop);

        let state = StackState::<i64, ()>::new();
        let shared = Shared {
            // SAFETY: address-of a live local.
            state: unsafe { NonNull::new_unchecked(std::ptr::from_ref(&state).cast_mut()) },
            armed: AtomicBool::new(true),
        };
        // SAFETY: vtable functions are sound for the test's `Shared` layout.
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::from_ref(&shared).cast(), &RACE_VTABLE)) };

        let mut future = ThunkFuture::new(&state);
        let mut cx = Context::from_waker(&waker);

        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, 123),
            Poll::Pending => panic!("expected Ready via post-set_waker re-check"),
        }
    }

    #[test]
    fn debug_impl() {
        let state = StackState::<u32, ()>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
        state.mark_worker_done();
        let future = ThunkFuture::new(&state);
        let debug = format!("{future:?}");
        assert!(debug.contains("ThunkFuture"));
    }

    #[test]
    fn unpin_trait() {
        fn assert_unpin<T: Unpin>() {}
        assert_unpin::<ThunkFuture<'_, u32, ()>>();
    }

    #[test]
    fn poll_with_complex_return_type() {
        let state = StackState::<Vec<String>, ()>::new();
        let data = vec![String::from("a"), String::from("b")];
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(data) };
        state.mark_worker_done();

        let mut future = ThunkFuture::new(&state);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, vec!["a", "b"]),
            Poll::Pending => panic!("expected Ready"),
        }
    }

    #[test]
    fn multiple_pending_polls_before_ready() {
        let state = StackState::<u32, ()>::new();
        let mut future = ThunkFuture::new(&state);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        for _ in 0..5 {
            assert!(Pin::new(&mut future).poll(&mut cx).is_pending());
        }

        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(100) };
        state.mark_worker_done();
        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, 100),
            Poll::Pending => panic!("expected Ready"),
        }
    }
}
