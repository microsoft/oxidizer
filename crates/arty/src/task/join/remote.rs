// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedReceiver;
use performables::arc::Arc;
use pin_project::pin_project;

use super::JoinError;
use crate::runtime::blocking_worker::is_current_blocking_pool;
use crate::task::execution::TaskResult;

/// A handle for receiving an async or blocking task's result.
///
/// Await the handle inside async code. Completion produces `Ok(result)`. A task
/// panic or shutdown cancellation produces [`JoinError`] without unwinding the
/// joining task.
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
/// #[arty::main]
/// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
///     let task = cx.scheduler().spawn(async |_| 42);
///     assert_eq!(task.await?, 42);
///     Ok(())
/// }
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
    result_rx: Option<BoxedReceiver<TaskResult<R>>>,
    blocking_pool: Option<Arc<()>>,
    completed: bool,
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(in crate::task) fn new(result_rx: BoxedReceiver<TaskResult<R>>) -> Self {
        Self {
            result_rx: Some(result_rx),
            blocking_pool: None,
            completed: false,
        }
    }

    pub(crate) fn shutdown() -> Self {
        Self {
            result_rx: None,
            blocking_pool: None,
            completed: false,
        }
    }

    pub(crate) fn with_blocking_pool(mut self, pool: Arc<()>) -> Self {
        self.blocking_pool = Some(pool);
        self
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
        let mut this = self.project();
        assert!(!*this.completed, "JoinHandle polled after completion");
        assert!(
            !this.blocking_pool.as_ref().is_some_and(is_current_blocking_pool),
            "blocking JoinHandle cannot be polled while the current blocking pool waits for it"
        );
        let Some(result_rx) = this.result_rx.as_mut().as_pin_mut() else {
            *this.completed = true;
            return Poll::Ready(Err(JoinError::shutdown()));
        };
        match result_rx.poll(cx) {
            Poll::Ready(Ok(result)) => {
                *this.completed = true;
                match result {
                    TaskResult::Completed(value) => Poll::Ready(Ok(value)),
                    TaskResult::Panicked(panic) => Poll::Ready(Err(JoinError::panicked(panic))),
                }
            }
            Poll::Ready(Err(_)) => {
                *this.completed = true;
                Poll::Ready(Err(JoinError::shutdown()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
