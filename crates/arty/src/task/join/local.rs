// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{self, Poll};

use events_once::{BoxedLocalReceiver, LocalEvent};
use pin_project::pin_project;

use super::JoinError;
use crate::task::execution::TaskResult;

/// A worker-local handle for receiving a task's result.
///
/// Returned by [`LocalTaskScheduler::spawn`](crate::task::LocalTaskScheduler::spawn).
/// Await it on the worker that created it. Both the handle and its result may
/// be non-[`Send`]; the handle cannot be sent to another thread.
///
/// Completion produces `Ok(result)`. A task panic, shutdown cancellation, or
/// rejection produces [`JoinError`]. Dropping the handle does not cancel its task.
///
/// # Panics
///
/// Panics if polled again after its result has been received.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::main]
/// async fn main(cx: arty::runtime::Builtins) -> Result<(), arty::task::JoinError> {
///     use std::rc::Rc;
///
///     let scheduler = cx
///         .local_scheduler()
///         .expect("the task runs on its associated worker");
///     let task = scheduler.spawn(async || Rc::new(42));
///     assert_eq!(*task.await?, 42);
///     Ok(())
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
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
