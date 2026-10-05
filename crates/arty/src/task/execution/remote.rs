// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{self, Poll};

use events_once::BoxedSender;
use observed::context::Transfer;
use observed::{Sink, emit};
use pin_project::{pin_project, pinned_drop};

use crate::runtime::telemetry::events::{TaskPanicked, TaskSucceeded};
use crate::task::execution::TaskResult;
use crate::task::execution::storage::{TaskStorage, discard_panic};

/// Wraps an inner future to be executed as a remote task.
///
/// We use this wrapper to add standard functionality to all tasks (e.g. panic handling).
#[pin_project(PinnedDrop)]
pub(super) struct RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    #[pin]
    inner: TaskStorage<F>,

    parent_task_enrichment: Transfer,

    /// Sink for completion/panic telemetry, routed to the runtime's `observed` destination.
    sink: Sink,

    /// Becomes `None` once a result has been sent.
    result_tx: Option<BoxedSender<TaskResult<R>>>,
}

impl<F, R> RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    pub(super) fn new(inner: F, result_tx: BoxedSender<TaskResult<R>>, parent_task_enrichment: Transfer, sink: Sink) -> Self {
        Self {
            inner: TaskStorage::new(inner),
            parent_task_enrichment,
            sink,
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
        if !this.inner.is_live() {
            return;
        }
        let _guard = this.parent_task_enrichment.apply_current_thread();
        if let Err(panic) = this.inner.destroy_pinned() {
            emit!(this.sink, TaskPanicked);
            discard_panic(panic);
        }
    }
}

impl<F, R> Future for RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.parent_task_enrichment.apply_current_thread();

        // We AssertUnwindSafe here because we consider the task completed on panic, which means
        // it will never be polled again - whatever it did to its internal state is now
        // irrelevant and if it corrupted some shared state, that is not really something we
        // can do anything about (a conscientious service will abort on panic to avoid that).
        let inner_poll_result = catch_unwind(AssertUnwindSafe(|| this.inner.poll(cx)));

        match inner_poll_result {
            Ok(Poll::Ready(result)) => {
                // An abandoned join destroys the result here, on the task's worker.
                let sender = this.result_tx.take().expect("future polled after completion");
                if let Err(panic) = catch_unwind(AssertUnwindSafe(|| sender.send(TaskResult::Completed(result)))) {
                    // The task completed successfully; a receiver notification panic must not
                    // change that outcome or its telemetry classification.
                    discard_panic(panic);
                }
                emit!(this.sink, TaskSucceeded);
                Poll::Ready(())
            }
            Ok(Poll::Pending) => Poll::Pending,
            Err(panic) => {
                if let Err(disposal) = catch_unwind(AssertUnwindSafe(|| {
                    if let Some(sender) = this.result_tx.take() {
                        sender.send(TaskResult::Panicked(panic));
                    }
                })) {
                    discard_panic(disposal);
                }

                emit!(this.sink, TaskPanicked);

                Poll::Ready(())
            }
        }
    }
}
