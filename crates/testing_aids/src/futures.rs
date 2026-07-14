// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for driving futures by hand in unit tests.
//!
//! [`FutureTestExt`] adds consuming manual-polling assertions
//! (`unwrap_ready`, `unwrap_pending`, `unwrap_ready_within`, `unwrap_ready_after`,
//! `unwrap_pending_for`) that let a test poll a future a fixed number of times and
//! assert the outcome, without spinning up an async runtime. The lower-level
//! [`poll_once`] primitive performs exactly one poll of an unpinned future.
//!
//! # Example
//!
//! ```
//! use testing_aids::FutureTestExt;
//!
//! assert_eq!(async { 7 }.unwrap_ready(), 7);
//! ```

use std::fmt::Debug;
use std::task::{Context, Poll};

/// Manually polls a future exactly once.
pub fn poll_once<F>(future: &mut F) -> Poll<F::Output>
where
    F: Future + Unpin,
{
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    future.as_mut().poll(&mut cx)
}

/// Extension trait adding consuming manual-polling assertions for futures in
/// tests.
///
/// Every method takes the future **by value** and pins it internally, so the
/// helpers work directly on `!Unpin` futures (such as the future returned by an
/// `async fn`) with no manual `pin!`. They suit the common case where a test
/// drives a future to a known outcome in a single call.
///
/// For step-by-step polling that inspects external state between polls (or
/// polls the same future across threads), use the lower-level [`poll_once`]
/// function directly.
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
    ///
    /// # Examples
    ///
    /// ```
    /// use testing_aids::FutureTestExt;
    ///
    /// assert_eq!(async { "done" }.unwrap_ready(), "done");
    /// ```
    fn unwrap_ready(self) -> Self::Output {
        let mut fut = std::pin::pin!(self);
        match poll_once(&mut fut) {
            Poll::Ready(value) => value,
            Poll::Pending => {
                panic!("expected future to be Ready after one poll, but it was Pending")
            }
        }
    }

    /// Polls the future exactly once, asserting it is `Pending`, then drops it.
    ///
    /// # Panics
    ///
    /// Panics if the future returns [`Poll::Ready`].
    ///
    /// # Examples
    ///
    /// ```
    /// use testing_aids::FutureTestExt;
    ///
    /// std::future::pending::<()>().unwrap_pending();
    /// ```
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
    /// Panics with `timeout_msg` if the future never completes within
    /// `max_polls` polls.
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
    fn unwrap_ready_after(self, n_pending: usize, message_if_not_pending: &str) -> Self::Output
    where
        Self::Output: Debug,
    {
        let mut fut = std::pin::pin!(self);
        for i in 0..n_pending {
            match poll_once(&mut fut) {
                Poll::Pending => {}
                Poll::Ready(value) => panic!(
                    "{message_if_not_pending}: got Ready({value:?}) after {} polls, expected Pending",
                    i + 1
                ),
            }
        }

        match poll_once(&mut fut) {
            Poll::Ready(value) => value,
            Poll::Pending => {
                panic!("expected Ready after {} polls, but got Pending", n_pending + 1)
            }
        }
    }

    /// Polls `n` times, asserting each poll returns `Pending`, then drops the
    /// future.
    ///
    /// # Panics
    ///
    /// Panics (using `message`) if any poll returns `Ready`.
    fn unwrap_pending_for(self, n: usize, message: &str) {
        let mut fut = std::pin::pin!(self);
        for _ in 0..n {
            assert!(matches!(poll_once(&mut fut), Poll::Pending), "{message}");
        }
    }
}

impl<F: Future> FutureTestExt for F {}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::pin::Pin;

    use super::*;

    /// A future returning a scripted sequence of polls, defaulting to `Pending`
    /// once the script is exhausted.
    struct ScriptedFuture {
        steps: VecDeque<Poll<u32>>,
    }

    impl ScriptedFuture {
        fn new(steps: Vec<Poll<u32>>) -> Self {
            Self {
                steps: VecDeque::from(steps),
            }
        }
    }

    impl Future for ScriptedFuture {
        type Output = u32;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
            self.steps.pop_front().unwrap_or(Poll::Pending)
        }
    }

    #[test]
    fn unwrap_ready_returns_value() {
        assert_eq!(ScriptedFuture::new(vec![Poll::Ready(42)]).unwrap_ready(), 42);
    }

    #[test]
    fn unwrap_ready_works_on_non_unpin_future() {
        // `async {}` futures are `!Unpin`; `unwrap_ready` pins internally.
        assert_eq!(async { 11u32 }.unwrap_ready(), 11);
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
    #[should_panic(expected = "expected Pending")]
    fn unwrap_ready_after_panics_when_early_ready() {
        let fut = ScriptedFuture::new(vec![Poll::Ready(1)]);
        let _ = fut.unwrap_ready_after(2, "expected Pending");
    }

    #[test]
    fn unwrap_pending_for_passes() {
        ScriptedFuture::new(vec![Poll::Pending, Poll::Pending]).unwrap_pending_for(2, "should stay pending");
    }

    #[test]
    #[should_panic(expected = "should stay pending")]
    fn unwrap_pending_for_panics_on_ready() {
        ScriptedFuture::new(vec![Poll::Ready(0)]).unwrap_pending_for(1, "should stay pending");
    }
}
