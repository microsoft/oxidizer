// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use arty_executor::remote::{JoinHandle as RemoteJoinHandle, Pool as RemotePool};
use pin_project::pin_project;

use super::JoinError;

/// A handle for receiving an async or blocking task's result.
///
/// Await the handle inside async code. Completion produces `Ok(result)`. A task
/// panic or shutdown cancellation produces [`JoinError`] without unwinding the
/// joining task.
///
/// For blocking tasks, panic containment covers the callback body. It does not
/// cover panics from capture or result destructors or from the receiver waker.
///
/// Dropping the handle does not cancel its task or rethrow a task panic.
/// The runtime must remain running for pending async work to complete.
/// If the task returns its own `Result<T, E>`, joining it produces
/// `Result<Result<T, E>, JoinError>`.
///
/// # Panics
///
/// Panics if polled again after its result has been received. A blocking
/// handle also panics if it is polled directly from a callback running in the
/// same blocking pool.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
/// # async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
/// let task = cx.scheduler().spawn(async |_| 42);
/// assert_eq!(task.await?, 42);
/// # Ok(())
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[derive(derive_more::Debug)]
#[pin_project]
pub struct JoinHandle<R>
where
    R: Send + 'static,
{
    #[debug(ignore)]
    #[pin]
    inner: RemoteJoinHandle<R>,
    completed: bool,
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(crate) fn new(inner: RemoteJoinHandle<R>) -> Self {
        Self { inner, completed: false }
    }

    pub(crate) fn shutdown(pool: &RemotePool) -> Self {
        let (sender, inner) = pool.channel();
        drop(sender);
        Self::new(inner)
    }

    #[cfg(test)]
    pub(crate) fn join(self) -> Result<R, JoinError> {
        futures::executor::block_on(self)
    }
}

impl<R> Future for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        assert!(!*this.completed, "JoinHandle polled after completion");
        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                *this.completed = true;
                Poll::Ready(result.map_err(JoinError::from_executor))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
