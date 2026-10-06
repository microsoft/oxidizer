// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedReceiver;
use performables::arc::Arc;
use pin_project::pin_project;

use super::JoinError;
use crate::runtime::blocking_worker::{BlockingWaitContext, is_current_blocking_pool};
use crate::runtime::thread::assert_not_flagged;
use crate::task::execution::TaskResult;

/// A handle for receiving an async or blocking task's result.
///
/// Await the handle inside async code, or call [`wait`](Self::wait) from
/// synchronous code. Completion produces `Ok(result)`. A task panic or shutdown
/// cancellation produces [`JoinError`] without unwinding the joining caller.
///
/// Dropping the handle does not cancel its task or rethrow a task panic.
/// The runtime must remain running for pending async work to complete.
/// If the task returns its own `Result<T, E>`, joining it produces
/// `Result<Result<T, E>, JoinError>`.
///
/// # Panics
///
/// Panics if polled again after its result has been received. A blocking
/// handle also panics if it is polled from its own blocking callback or from
/// async work that a callback in the same pool is synchronously waiting for.
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
    #[debug(ignore)]
    blocking_wait: Option<Arc<BlockingWaitContext>>,
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
            blocking_wait: None,
            completed: false,
        }
    }

    pub(crate) fn shutdown() -> Self {
        Self {
            result_rx: None,
            blocking_pool: None,
            blocking_wait: None,
            completed: false,
        }
    }

    pub(crate) fn with_blocking_pool(mut self, pool: Arc<()>) -> Self {
        self.blocking_pool = Some(pool);
        self
    }

    pub(crate) fn with_blocking_wait_context(mut self, context: Arc<BlockingWaitContext>) -> Self {
        self.blocking_wait = Some(context);
        self
    }

    /// Blocks until the task's result is available.
    ///
    /// Use `.await` inside async code instead. This waits for one task,
    /// not for runtime shutdown. Stopping or dropping the runtime owner waits
    /// for its workers to stop.
    ///
    /// # Errors
    ///
    /// Returns [`JoinError`] if the task panicked or shutdown cancelled or
    /// rejected it. An error returned by the task itself remains its result.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been received by polling the handle.
    /// Also panics if called from an async Arty worker or while waiting for a
    /// task belonging to the current blocking pool, even if the result is
    /// already ready. A blocking callback may wait for an async task, but not
    /// for another task queued behind itself in the same pool, including
    /// through async descendants.
    ///
    /// An async task waited by a blocking callback fails with a panic
    /// [`JoinError`] if it or one of its descendants awaits blocking work from
    /// that callback's pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let task = runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 });
    /// assert_eq!(task.wait()?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn wait(self) -> Result<R, JoinError> {
        assert_not_flagged();
        assert!(
            !self.blocking_pool.as_ref().is_some_and(is_current_blocking_pool),
            "blocking JoinHandle::wait cannot wait for the current blocking pool"
        );
        let _blocking_wait = self.blocking_wait.as_ref().and_then(BlockingWaitContext::activate_current_pool);

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
