// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{self, Poll};

use arty_executor::{RemoteTaskSender, discard_task_panic_payload};
use pin_project::{pin_project, pinned_drop};

use crate::task::execution::storage::TaskStorage;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RemoteTaskOutcome {
    Succeeded,
    Panicked,
}

#[pin_project(PinnedDrop)]
pub(crate) struct RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    #[pin]
    inner: TaskStorage<F>,
    result_tx: Option<RemoteTaskSender<R>>,
}

impl<F, R> RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    pub(crate) fn new(inner: F, result_tx: RemoteTaskSender<R>) -> Self {
        Self {
            inner: TaskStorage::new(inner),
            result_tx: Some(result_tx),
        }
    }
}

#[pinned_drop]
impl<F, R> PinnedDrop for RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();
        let inner_panic = this.inner.destroy_pinned().err();
        if let Some(result_tx) = this.result_tx.take()
            && let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(result_tx)))
        {
            discard_task_panic_payload(panic);
        }
        if let Some(panic) = inner_panic {
            resume_unwind(panic);
        }
    }
}

impl<F, R> Future for RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    type Output = RemoteTaskOutcome;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        let inner_poll = catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx)));

        match inner_poll {
            Ok(Poll::Ready(result)) => {
                let sender = this.result_tx.take().expect("remote task polled after completion");
                if let Err(panic) = sender.completed(result) {
                    discard_task_panic_payload(panic);
                }
                Poll::Ready(RemoteTaskOutcome::Succeeded)
            }
            Ok(Poll::Pending) => Poll::Pending,
            Err(panic) => {
                if let Err(disposal) = this.inner.as_mut().destroy_pinned() {
                    discard_task_panic_payload(disposal);
                }
                if let Some(sender) = this.result_tx.take()
                    && let Err(disposal) = sender.panicked(panic)
                {
                    discard_task_panic_payload(disposal);
                }
                Poll::Ready(RemoteTaskOutcome::Panicked)
            }
        }
    }
}
