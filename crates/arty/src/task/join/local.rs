// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::resume_unwind;
use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedLocalReceiver;
use pin_project::pin_project;

use crate::task::execution::TaskResult;

/// The result of a task on its originating worker.
///
/// Await this handle on the worker that created it. Results need not be [`Send`],
/// and the handle cannot be sent to another thread.
///
/// Cancellation or rejection during shutdown leaves the handle pending indefinitely,
/// rather than returning a cancellation error. Dropping it does not cancel the
/// task or rethrow its panic elsewhere. See the
/// [documentation guides](crate#documentation) for shutdown coordination.
///
/// # Panics
///
/// The result may be obtained at most once, by awaiting the future.
/// Attempting to obtain the result multiple times will panic.
///
/// Resumes the original panic payload if the task panicked while unwinding was enabled.
#[derive(derive_more::Debug)]
#[pin_project]
pub struct LocalJoinHandle<R: 'static> {
    #[debug(ignore)]
    #[pin]
    result_rx: BoxedLocalReceiver<TaskResult<R>>,

    disconnected: bool,
}

impl<R: 'static> LocalJoinHandle<R> {
    pub(in crate::task) fn new(result_rx: BoxedLocalReceiver<TaskResult<R>>) -> Self {
        Self {
            result_rx,
            disconnected: false,
        }
    }
}

impl<R: 'static> Future for LocalJoinHandle<R> {
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
