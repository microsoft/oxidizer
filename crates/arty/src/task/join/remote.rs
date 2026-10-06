// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedReceiver;
use pin_project::pin_project;

use super::JoinError;
use crate::runtime::blocking_worker::assert_not_current_blocking_task;
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
/// Panics if polled again after its result has been received.
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
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(in crate::task) fn new(result_rx: BoxedReceiver<TaskResult<R>>) -> Self {
        Self {
            result_rx: Some(result_rx),
        }
    }

    pub(crate) fn shutdown() -> Self {
        Self { result_rx: None }
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
    /// Also panics if called from an async Arty worker or a blocking callback,
    /// even if the result is already ready. Blocking callbacks cannot wait
    /// for work through a blocking pool without risking pool starvation.
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
        assert_not_current_blocking_task();

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
        let Some(result_rx) = this.result_rx.as_mut().as_pin_mut() else {
            return Poll::Ready(Err(JoinError::shutdown()));
        };
        match result_rx.poll(cx) {
            Poll::Ready(Ok(result)) => match result {
                TaskResult::Completed(value) => Poll::Ready(Ok(value)),
                TaskResult::Panicked(panic) => Poll::Ready(Err(JoinError::panicked(panic))),
            },
            Poll::Ready(Err(_)) => Poll::Ready(Err(JoinError::shutdown())),
            Poll::Pending => Poll::Pending,
        }
    }
}
