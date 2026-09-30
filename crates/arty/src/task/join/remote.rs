// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::resume_unwind;
use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedReceiver;
use pin_project::pin_project;

use crate::runtime::thread::assert_not_flagged;
use crate::task::execution::TaskResult;

/// The result of an asynchronous or blocking task.
///
/// Await the handle from asynchronous code, or use [`wait`](Self::wait) from a
/// blocking-safe thread. The result is returned directly, not wrapped in a
/// task-status `Result`.
///
/// Cancellation, including runtime shutdown before an asynchronous task completes,
/// leaves the handle pending indefinitely. It does not return a cancellation error.
/// Dropping the handle does not cancel the task or rethrow its panic elsewhere.
/// See the [documentation guides](crate#documentation) before coordinating
/// joins with shutdown.
///
/// # Panics
///
/// The result may be obtained at most once, either by awaiting the future or by calling `wait()`.
/// Attempting to obtain the result multiple times will panic.
///
/// Resumes the original panic payload if the task panicked while unwinding was enabled.
#[derive(derive_more::Debug)]
#[pin_project]
pub struct JoinHandle<R>
where
    R: Send + 'static,
{
    #[debug(ignore)]
    #[pin]
    result_rx: BoxedReceiver<TaskResult<R>>,

    disconnected: bool,
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    pub(in crate::task) fn new(result_rx: BoxedReceiver<TaskResult<R>>) -> Self {
        Self {
            result_rx,
            disconnected: false,
        }
    }

    /// Synchronously waits for the task to complete, returning the result.
    ///
    /// A cancelled or rejected task leaves this method blocked indefinitely.
    /// This is not a shutdown wait; use [`Runtime::wait`](crate::runtime::Runtime::wait)
    /// to wait for workers to stop.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been obtained either via `wait()` or by awaiting.
    ///
    /// Panics if called from an asynchronous Arty worker. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
    /// Also resumes a panic transported from the task.
    #[expect(
        clippy::must_use_candidate,
        reason = "caller might not care about result - this is a generic wrapper"
    )]
    pub fn wait(self) -> R {
        assert_not_flagged();

        futures::executor::block_on(self)
    }
}

impl<R> Future for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        if self.disconnected {
            return Poll::Pending;
        }

        let this = self.project();

        match this.result_rx.poll(cx) {
            Poll::Ready(Ok(result)) => match result {
                TaskResult::Completed(value) => Poll::Ready(value),
                TaskResult::Panicked(panic) => resume_unwind(panic),
            },
            Poll::Ready(Err(_)) => {
                // This typically means the runtime is shutting down. When this happens, the join
                // handle will never complete. While this creates a certain risk of resource leaks
                // it still seems better than punishing arbitrary `spawn().await` calls in with
                // a panic just because they are issued at the moment of shutdown (avoiding that
                // panic would imply excessive coordination and such a panic might be difficult
                // to differentiate from "things actually going wrong").
                *this.disconnected = true;
                Poll::Pending
            }
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

        assert_eq!(handle.wait(), 123);
    }
}
