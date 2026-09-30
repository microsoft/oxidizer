// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use events_once::{BoxedLocalReceiver, LocalEvent};
use pin_project::pin_project;

use super::JoinError;
use crate::task::execution::TaskResult;

/// The result of a task on its originating worker.
///
/// Await this handle on the worker that created it. Results need not be [`Send`],
/// and the handle cannot be sent to another thread.
///
/// Task panics and shutdown cancellation/rejection return [`JoinError`]. Dropping
/// the handle does not cancel the task. See the
/// [documentation guides](crate#documentation) for shutdown coordination.
///
/// # Panics
///
/// The result may be obtained at most once, by awaiting the future.
/// Attempting to obtain the result multiple times will panic.
#[derive(derive_more::Debug)]
#[pin_project]
pub struct LocalJoinHandle<R: 'static> {
    #[debug(ignore)]
    #[pin]
    result_rx: BoxedLocalReceiver<TaskResult<R>>,
}

impl<R: 'static> LocalJoinHandle<R> {
    pub(in crate::task) fn new(result_rx: BoxedLocalReceiver<TaskResult<R>>) -> Self {
        Self { result_rx }
    }

    pub(crate) fn shutdown() -> Self {
        let (sender, receiver) = LocalEvent::boxed();
        drop(sender);
        Self::new(receiver)
    }

    pub(crate) fn panicked(payload: Box<dyn std::any::Any + Send + 'static>) -> Self {
        let (sender, receiver) = LocalEvent::boxed();
        sender.send(TaskResult::Panicked(payload));
        Self::new(receiver)
    }
}

impl<R: 'static> Future for LocalJoinHandle<R> {
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
