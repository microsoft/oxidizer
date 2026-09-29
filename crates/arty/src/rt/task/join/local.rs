// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::resume_unwind;
use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedLocalReceiver;
use pin_project::pin_project;

use crate::rt::task::execution::TaskResult;

/// Enables the caller to obtain a result from a task running on the current worker thread.
///
/// Spawning a task supplies the caller a join handle for the task.
///
/// # Panics
///
/// The result may be obtained at most once, by awaiting the future.
/// Attempting to obtain the result multiple times will panic.
///
/// Re-throws any panic from the associated task if the task ended with a panic.
#[derive(derive_more::Debug)]
#[pin_project]
pub struct LocalJoinHandle<R: 'static> {
    #[debug(ignore)]
    #[pin]
    result_rx: BoxedLocalReceiver<TaskResult<R>>,

    disconnected: bool,
}

impl<R: 'static> LocalJoinHandle<R> {
    pub(in crate::rt::task) fn new(result_rx: BoxedLocalReceiver<TaskResult<R>>) -> Self {
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
