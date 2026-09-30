// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use events_once::{BoxedReceiver, Event};
use pin_project::pin_project;

use super::JoinError;
use crate::runtime::thread::assert_not_flagged;
use crate::task::execution::TaskResult;

/// The result of an asynchronous or blocking task.
///
/// Await the handle from asynchronous code, or use [`wait`](Self::wait) from a
/// blocking-safe thread. Completion returns `Ok(result)`; a task panic or shutdown
/// returns [`JoinError`] without unwinding the joining caller.
///
/// Cancellation, including runtime shutdown before an asynchronous task completes,
/// returns an error for which [`JoinError::is_shutdown`] is `true`.
/// Dropping the handle does not cancel the task.
/// See the [documentation guides](crate#documentation) before coordinating
/// joins with shutdown.
///
/// # Panics
///
/// The result may be obtained at most once, either by awaiting the future or by calling `wait()`.
/// Attempting to obtain the result multiple times will panic.
#[derive(derive_more::Debug)]
#[pin_project]
pub struct JoinHandle<R>
where
    R: Send + 'static,
{
    #[debug(ignore)]
    #[pin]
    result_rx: BoxedReceiver<TaskResult<R>>,
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(in crate::task) fn new(result_rx: BoxedReceiver<TaskResult<R>>) -> Self {
        Self { result_rx }
    }

    pub(crate) fn shutdown() -> Self {
        let (sender, receiver) = Event::boxed();
        drop(sender);
        Self::new(receiver)
    }

    /// Synchronously waits for the task to complete, returning the result.
    ///
    /// A cancelled or rejected task returns [`JoinError`].
    /// This is not a shutdown wait; use [`Runtime::wait`](crate::runtime::Runtime::wait)
    /// to wait for workers to stop.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been obtained either via `wait()` or by awaiting.
    ///
    /// Panics if called from an asynchronous Arty worker. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    ///
    /// # Errors
    ///
    /// Returns an error if the task panicked or was cancelled/rejected during shutdown.
    pub fn wait(self) -> Result<R, JoinError> {
        assert_not_flagged();

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

        match this.result_rx.poll(cx) {
            Poll::Ready(Ok(result)) => match result {
                TaskResult::Completed(value) => Poll::Ready(Ok(value)),
                TaskResult::Panicked(panic) => Poll::Ready(Err(JoinError::panicked(panic))),
            },
            Poll::Ready(Err(_)) => Poll::Ready(Err(JoinError::shutdown())),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::num::NonZeroUsize;

    use crate::runtime::Runtime;
    use crate::runtime::config::ProcessorCount;

    // Processor discovery uses native hardware APIs unavailable under Miri.
    #[cfg_attr(miri, ignore = "native runtime construction; result transport is tested independently")]
    #[test]
    fn spawned_task_delivers_its_result() {
        let runtime = Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
            .build()
            .expect("runtime");

        let handle = runtime.task_scheduler().spawn(async |_cx| 123u32);

        assert_eq!(handle.wait().unwrap(), 123);
    }
}
