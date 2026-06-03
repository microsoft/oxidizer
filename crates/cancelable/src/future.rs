// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Future extension for cooperative cancellation
//!
//! The [`CancelableExt`] trait adds a
//! [`cancelable`](CancelableExt::cancelable) method
//! to any [`Future`], pairing it with a [`CancellationToken`] so that each
//! poll checks for cancellation before and after driving the inner future.
//!
//! ```
//! # async fn example() -> Result<(), ohno::AppError> {
//! use cancelable::{CancelableExt, CancellationTokenSource};
//!
//! let source = CancellationTokenSource::new();
//! let token = source.token();
//!
//! let result = async { 42 }.cancelable(token).await?;
//! assert_eq!(result, 42);
//! # Ok(())
//! # }
//! ```

use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project::pin_project;

use crate::CancellationToken;

/// Error returned when a future is canceled
#[ohno::error]
#[display("operation was canceled")]
pub struct Canceled {}

/// Extension trait that adds cancellation support to any [`Future`].
pub trait CancelableExt: Future + Sized {
    /// Wraps this future so that each poll checks the given [`CancellationToken`]:
    ///
    /// - If the token is canceled (before *or* after polling the inner
    ///   future), the combined future resolves to <code>Err([Canceled])</code>.
    /// - Otherwise the inner future's output is forwarded as `Ok(output)`.
    ///
    /// # Note on wake semantics
    ///
    /// Cancellation is checked cooperatively: the extension inspects the token
    /// each time the inner future is polled.  If the inner future is pending
    /// and nothing else wakes the task, cancellation will not be noticed until
    /// the next poll.  This mirrors the cooperative model of the `C#` method
    /// `CancellationToken.ThrowIfCancellationRequested()`.
    ///
    /// # Examples
    ///
    /// Successful completion:
    ///
    /// ```
    /// # async fn example() {
    /// use cancelable::{CancelableExt, CancellationTokenSource};
    ///
    /// let source = CancellationTokenSource::new();
    /// let result = async { "hello" }.cancelable(source.token()).await;
    /// assert_eq!(result.unwrap(), "hello");
    /// # }
    /// ```
    ///
    /// Cancelled before first poll:
    ///
    /// ```
    /// # async fn example() {
    /// use cancelable::{CancelableExt, CancellationTokenSource};
    ///
    /// let source = CancellationTokenSource::new();
    /// source.cancel();
    ///
    /// let result = async { unreachable!() }.cancelable(source.token()).await;
    /// assert!(result.unwrap_err().to_string().contains("canceled"));
    /// # }
    /// ```
    fn cancelable(self, token: CancellationToken) -> Cancelable<Self>;
}

impl<F: Future> CancelableExt for F {
    fn cancelable(self, token: CancellationToken) -> Cancelable<Self> {
        Cancelable { inner: self, token }
    }
}

/// Future returned by
/// [`cancelable`](CancelableExt::cancelable).
///
/// See the trait method documentation for semantics.
#[derive(Debug)]
#[pin_project]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Cancelable<F> {
    #[pin]
    inner: F,
    token: CancellationToken,
}

impl<F: Future> Future for Cancelable<F> {
    type Output = Result<F::Output, Canceled>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // Check cancellation before running the inner future so we can
        // short-circuit without performing unnecessary work.
        if this.token.is_cancelled() {
            return Poll::Ready(Err(Canceled::new()));
        }

        match this.inner.poll(cx) {
            Poll::Ready(output) => Poll::Ready(Ok(output)),
            Poll::Pending => {
                // Check for cancellation again, now that we've spent time running the inner future.
                if this.token.is_cancelled() {
                    Poll::Ready(Err(Canceled::new()))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tick::{Clock, FutureExt};

    use super::*;
    use crate::CancellationTokenSource;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn completed_future_returns_ok() {
        let source = CancellationTokenSource::new();
        let result = async { 42 }.cancelable(source.token()).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn cancelled_future_returns_err() {
        let source = CancellationTokenSource::new();
        source.cancel();

        let result = async { unreachable!("should not poll inner future") }
            .cancelable(source.token())
            .await;
        assert!(result.unwrap_err().to_string().contains("canceled"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn cancellation_triggered_by_inner_future_leads_to_cancellation_error() {
        struct CancelOnPoll(CancellationTokenSource);
        impl Future for CancelOnPoll {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
                self.0.cancel();
                Poll::Pending
            }
        }

        let clock = Clock::new_tokio();
        let source = CancellationTokenSource::new();
        let token = source.token();
        let message = CancelOnPoll(source)
            .timeout(&clock, std::time::Duration::from_secs(5))
            .cancelable(token)
            .await
            .expect_err("should fail")
            .to_string();
        assert!(message.contains("canceled"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn already_cancelled_token() {
        let clock = Clock::new_tokio();
        let message = async { unreachable!() }
            .timeout(&clock, std::time::Duration::from_secs(5))
            .cancelable(CancellationToken::cancelled())
            .await
            .expect_err("should fail")
            .to_string();
        assert!(message.contains("canceled"));
    }
}
