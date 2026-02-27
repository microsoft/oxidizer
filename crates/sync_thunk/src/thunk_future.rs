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
            assert!(
                !self.state.has_panicked(),
                "thunked function panicked on worker thread"
            );
            // SAFETY: The worker has signalled completion via the atomic store.
            // We are the only consumer of the result slot.
            let val = unsafe { self.state.take_result().expect("guarded by ready flag to always contain a result") };
            return Poll::Ready(val);
        }

        // Register the waker before re-checking readiness to avoid lost wakeups.
        self.state.set_waker(cx.waker().clone());

        // Re-check after waker registration: the worker may have completed
        // between our first is_ready() check and the set_waker() call above.
        if self.state.is_ready() {
            assert!(
                !self.state.has_panicked(),
                "thunked function panicked on worker thread"
            );
            // SAFETY: Same as above — ready flag guarantees result is present.
            let val = unsafe { self.state.take_result().expect("guarded by ready flag to always contain a result") };
            return Poll::Ready(val);
        }

        Poll::Pending
    }
}

#[cfg(test)]
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

        let mut future = ThunkFuture::new(&state);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, -1),
            Poll::Pending => panic!("expected Ready"),
        }
    }

    #[test]
    fn debug_impl() {
        let state = StackState::<u32, ()>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
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
        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(val) => assert_eq!(val, 100),
            Poll::Pending => panic!("expected Ready"),
        }
    }
}
