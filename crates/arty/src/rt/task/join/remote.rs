// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::resume_unwind;
use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedReceiver;
use pin_project::pin_project;

use crate::rt::runtime::thread::assert_not_flagged;
use crate::rt::task::execution::TaskResult;

/// Enables the caller to obtain a result from a task running on an unspecified worker thread.
///
/// Spawning a task supplies the caller a join handle for the task.
///
/// # Panics
///
/// The result may be obtained at most once, either by awaiting the future or by calling `wait()`.
/// Attempting to obtain the result multiple times will panic.
///
/// Re-throws any panic from the associated task if the task ended with a panic.
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
    pub(in crate::rt::task) fn new(result_rx: BoxedReceiver<TaskResult<R>>) -> Self {
        Self {
            result_rx,
            disconnected: false,
        }
    }

    /// Synchronously waits for the task to complete, returning the result.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been obtained either via `wait()` or by awaiting.
    ///
    /// Panics if called from a thread owned by the Arty runtime. This function is only intended
    /// to be called from a blocking-safe context such as `fn main()` or a `#[test]` entry point.
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
mod tests {
    use std::num::NonZeroUsize;

    use crate::rt::runtime::config::ProcessorCount;
    use crate::rt::runtime::handle::Runtime;

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
