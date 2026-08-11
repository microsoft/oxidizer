// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utilities for driving futures by hand in unit tests.
//!
//! [`FutureTestExt`] adds consuming manual-polling assertions (`unwrap_ready`,
//! `unwrap_pending`, `unwrap_ready_within`, `unwrap_ready_after`,
//! `unwrap_pending_for`) that let a test poll a future a fixed number of times
//! and assert the outcome, without spinning up an async runtime. The
//! lower-level [`poll_once`] primitive performs exactly one poll of an unpinned
//! future.

use std::task::{Context, Poll, Waker};

/// Polls a future exactly once with a no-op waker.
///
/// The future must be [`Unpin`], because it is polled through a mutable
/// reference the caller keeps across polls. Futures produced by `async fn` and
/// `async {}` blocks are `!Unpin`, so either pin them first (for example with
/// [`std::pin::pin!`]) or use the [`FutureTestExt`] methods, which take the
/// future by value and pin it internally.
///
/// Use this when a test needs to inspect external state between polls, or poll
/// the same future from several places. For the common
/// "drive to a known outcome" case, prefer [`FutureTestExt`].
pub fn poll_once<F>(future: &mut F) -> Poll<F::Output>
where
    F: Future + Unpin,
{
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    future.as_mut().poll(&mut cx)
}

/// Extension trait adding consuming manual-polling assertions for futures in tests.
///
/// Every method takes the future **by value** and pins it internally, so the
/// helpers work directly on `!Unpin` futures (such as the future returned by an
/// `async fn`) with no manual `pin!`.
///
/// # Examples
///
/// ```
/// use testing_aids::FutureTestExt;
///
/// assert_eq!(async { 7 }.unwrap_ready(), 7);
/// ```
pub trait FutureTestExt: Future + Sized {
    /// Polls the future exactly once and returns its output, panicking if it is
    /// still `Pending`.
    ///
    /// # Panics
    ///
    /// Panics if the future returns [`Poll::Pending`].
    #[track_caller]
    fn unwrap_ready(self) -> Self::Output {
        let mut fut = std::pin::pin!(self);
        match poll_once(&mut fut) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("expected future to be Ready after one poll, but it was Pending"),
        }
    }

    /// Polls the future exactly once, asserting it is `Pending`, then drops it.
    ///
    /// # Panics
    ///
    /// Panics if the future returns [`Poll::Ready`].
    #[track_caller]
    fn unwrap_pending(self) {
        let mut fut = std::pin::pin!(self);
        assert!(
            poll_once(&mut fut).is_pending(),
            "expected future to be Pending after one poll, but it was Ready"
        );
    }

    /// Polls up to `max_polls` times, returning the first `Ready` output.
    ///
    /// # Panics
    ///
    /// Panics with `timeout_msg` if the future never completes within `max_polls` polls.
    #[track_caller]
    fn unwrap_ready_within(self, max_polls: usize, timeout_msg: &str) -> Self::Output {
        let mut fut = std::pin::pin!(self);
        for _ in 0..max_polls {
            if let Poll::Ready(value) = poll_once(&mut fut) {
                return value;
            }
        }
        panic!("{timeout_msg}");
    }

    /// Polls exactly `n_pending` times expecting `Pending`, then once more
    /// expecting `Ready`, returning the output.
    ///
    /// This makes timing expectations explicit and catches off-by-one timing
    /// bugs in state machines.
    ///
    /// # Panics
    ///
    /// Panics (using `message_if_not_pending`) if any of the first `n_pending`
    /// polls returns `Ready`, or if the final poll is still `Pending`.
    #[track_caller]
    fn unwrap_ready_after(self, n_pending: usize, message_if_not_pending: &str) -> Self::Output {
        let mut fut = std::pin::pin!(self);
        for i in 0..n_pending {
            match poll_once(&mut fut) {
                Poll::Pending => {}
                Poll::Ready(_) => panic!("{message_if_not_pending}: got Ready after {} polls, expected Pending", i + 1),
            }
        }

        match poll_once(&mut fut) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!(
                "{message_if_not_pending}: expected Ready after {} polls, but got Pending",
                n_pending + 1
            ),
        }
    }

    /// Polls `n` times, asserting each poll returns `Pending`, then drops the future.
    ///
    /// # Panics
    ///
    /// Panics (using `message`) if any poll returns `Ready`, and panics if `n`
    /// is zero, because zero polls would assert nothing.
    #[track_caller]
    fn unwrap_pending_for(self, n: usize, message: &str) {
        assert!(n > 0, "unwrap_pending_for requires n > 0; zero polls would assert nothing");
        let mut fut = std::pin::pin!(self);
        for _ in 0..n {
            assert!(poll_once(&mut fut).is_pending(), "{message}");
        }
    }
}

impl<F: Future> FutureTestExt for F {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::rc::Rc;

    use super::*;

    /// A future returning a scripted sequence of polls.
    ///
    /// It counts every poll it receives and panics once the script is
    /// exhausted, so a helper that polls more times than it documents fails
    /// loudly instead of silently observing `Pending` forever.
    struct ScriptedFuture {
        steps: VecDeque<Poll<u32>>,
        polls: Rc<Cell<usize>>,
    }

    impl ScriptedFuture {
        fn new(steps: Vec<Poll<u32>>) -> Self {
            Self {
                steps: VecDeque::from(steps),
                polls: Rc::new(Cell::new(0)),
            }
        }

        /// Returns the future together with a handle that observes how many
        /// times it has been polled, so a test can assert the exact poll count
        /// after the helper has consumed the future.
        fn with_poll_count(steps: Vec<Poll<u32>>) -> (Self, Rc<Cell<usize>>) {
            let future = Self::new(steps);
            let polls = Rc::clone(&future.polls);
            (future, polls)
        }
    }

    impl Future for ScriptedFuture {
        type Output = u32;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
            self.polls.set(self.polls.get() + 1);
            self.steps
                .pop_front()
                .unwrap_or_else(|| panic!("ScriptedFuture polled after its script was exhausted"))
        }
    }

    #[test]
    fn poll_once_returns_ready_value() {
        let mut fut = ScriptedFuture::new(vec![Poll::Ready(5)]);
        assert_eq!(poll_once(&mut fut), Poll::Ready(5));
    }

    #[test]
    fn poll_once_returns_pending_and_can_be_repeated() {
        let (mut fut, polls) = ScriptedFuture::with_poll_count(vec![Poll::Pending, Poll::Ready(5)]);
        assert_eq!(poll_once(&mut fut), Poll::Pending);
        assert_eq!(poll_once(&mut fut), Poll::Ready(5));
        assert_eq!(polls.get(), 2, "each call must poll the future exactly once");
    }

    #[test]
    fn poll_once_accepts_a_pinned_async_block() {
        // `async {}` futures are `!Unpin`; pinning first satisfies `poll_once`,
        // as its doc comment instructs.
        let mut fut = std::pin::pin!(async { 3_u32 });
        assert_eq!(poll_once(&mut fut), Poll::Ready(3));
    }

    #[test]
    fn unwrap_ready_returns_value() {
        assert_eq!(ScriptedFuture::new(vec![Poll::Ready(42)]).unwrap_ready(), 42);
    }

    #[test]
    fn unwrap_ready_polls_exactly_once() {
        let (fut, polls) = ScriptedFuture::with_poll_count(vec![Poll::Ready(42)]);
        assert_eq!(fut.unwrap_ready(), 42);
        assert_eq!(polls.get(), 1);
    }

    #[test]
    fn unwrap_ready_works_on_non_unpin_future() {
        // `async {}` futures are `!Unpin`; `unwrap_ready` pins internally.
        assert_eq!(async { 11_u32 }.unwrap_ready(), 11);
    }

    #[test]
    #[should_panic(expected = "Ready after one poll")]
    fn unwrap_ready_panics_when_pending() {
        std::future::pending::<()>().unwrap_ready();
    }

    #[test]
    fn unwrap_pending_passes_when_pending() {
        ScriptedFuture::new(vec![Poll::Pending]).unwrap_pending();
    }

    #[test]
    fn unwrap_pending_polls_exactly_once() {
        let (fut, polls) = ScriptedFuture::with_poll_count(vec![Poll::Pending]);
        fut.unwrap_pending();
        assert_eq!(polls.get(), 1);
    }

    #[test]
    #[should_panic(expected = "Pending after one poll")]
    fn unwrap_pending_panics_when_ready() {
        ScriptedFuture::new(vec![Poll::Ready(3)]).unwrap_pending();
    }

    #[test]
    fn unwrap_ready_within_drives_to_completion() {
        let fut = ScriptedFuture::new(vec![Poll::Pending, Poll::Pending, Poll::Ready(7)]);
        assert_eq!(fut.unwrap_ready_within(10, "never finished"), 7);
    }

    #[test]
    fn unwrap_ready_within_stops_polling_once_ready() {
        let (fut, polls) = ScriptedFuture::with_poll_count(vec![Poll::Pending, Poll::Pending, Poll::Ready(7)]);
        assert_eq!(fut.unwrap_ready_within(10, "never finished"), 7);
        assert_eq!(polls.get(), 3, "must stop at the first Ready, not exhaust max_polls");
    }

    #[test]
    #[should_panic(expected = "never finished")]
    fn unwrap_ready_within_panics_on_timeout() {
        let fut = ScriptedFuture::new(vec![Poll::Pending, Poll::Pending]);
        let _ = fut.unwrap_ready_within(2, "never finished");
    }

    #[test]
    fn unwrap_ready_after_matches_schedule() {
        let fut = ScriptedFuture::new(vec![Poll::Pending, Poll::Pending, Poll::Ready(9)]);
        assert_eq!(fut.unwrap_ready_after(2, "should be pending"), 9);
    }

    #[test]
    fn unwrap_ready_after_polls_exactly_n_plus_one_times() {
        let (fut, polls) = ScriptedFuture::with_poll_count(vec![Poll::Pending, Poll::Pending, Poll::Ready(9)]);
        assert_eq!(fut.unwrap_ready_after(2, "should be pending"), 9);
        assert_eq!(polls.get(), 3);
    }

    /// An output type deliberately without a [`Debug`](std::fmt::Debug) impl.
    ///
    /// Any helper that requires one would fail to compile against this type,
    /// so it keeps the assertions usable with opaque outputs.
    struct NotDebug(u32);

    /// Returns `Pending` on its first poll, then `Ready`.
    struct PendingOnce {
        polled: bool,
    }

    impl Future for PendingOnce {
        type Output = NotDebug;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<NotDebug> {
            if self.polled {
                Poll::Ready(NotDebug(4))
            } else {
                self.polled = true;
                Poll::Pending
            }
        }
    }

    #[test]
    fn unwrap_ready_after_accepts_non_debug_output() {
        let value = PendingOnce { polled: false }.unwrap_ready_after(1, "should be pending once");
        assert_eq!(value.0, 4);
    }

    #[test]
    #[should_panic(expected = "expected Pending")]
    fn unwrap_ready_after_panics_when_early_ready() {
        let fut = ScriptedFuture::new(vec![Poll::Ready(1)]);
        let _ = fut.unwrap_ready_after(2, "expected Pending");
    }

    #[test]
    #[should_panic(expected = "custom context: expected Ready after 2 polls, but got Pending")]
    fn unwrap_ready_after_panics_when_never_ready() {
        let fut = ScriptedFuture::new(vec![Poll::Pending, Poll::Pending]);
        let _ = fut.unwrap_ready_after(1, "custom context");
    }

    #[test]
    fn unwrap_pending_for_passes() {
        ScriptedFuture::new(vec![Poll::Pending, Poll::Pending]).unwrap_pending_for(2, "should stay pending");
    }

    #[test]
    fn unwrap_pending_for_polls_exactly_n_times() {
        let (fut, polls) = ScriptedFuture::with_poll_count(vec![Poll::Pending, Poll::Pending]);
        fut.unwrap_pending_for(2, "should stay pending");
        assert_eq!(polls.get(), 2);
    }

    #[test]
    #[should_panic(expected = "should stay pending")]
    fn unwrap_pending_for_panics_on_ready() {
        ScriptedFuture::new(vec![Poll::Ready(0)]).unwrap_pending_for(1, "should stay pending");
    }

    #[test]
    #[should_panic(expected = "requires n > 0")]
    fn unwrap_pending_for_panics_on_zero_polls() {
        std::future::ready(1_u32).unwrap_pending_for(0, "should stay pending");
    }
}
