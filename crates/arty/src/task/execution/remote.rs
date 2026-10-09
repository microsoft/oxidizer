// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{self, Poll};

use events_once::BoxedSender;
use observed::context::Transfer;
use observed::{Sink, emit};
use performables::arc::Arc;
use pin_project::{pin_project, pinned_drop};

use crate::runtime::seismograph::TaskTelemetry;
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

    /// Task wrappers check shutdown again before invoking their deferred factory.
    shutdown_signal: Option<Arc<AtomicBool>>,

    telemetry: Option<TaskTelemetry>,
}

fn dispose_sender<F>(drop_sender: F)
where
    F: FnOnce(),
{
    if let Err(panic) = catch_unwind(AssertUnwindSafe(drop_sender)) {
        discard_panic(panic);
    }
}

impl<F, R> RemoteTaskFuture<F, R>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    #[cfg(test)]
    pub(super) fn new(inner: F, result_tx: BoxedSender<TaskResult<R>>, parent_task_enrichment: Transfer, sink: Sink) -> Self {
        Self {
            inner: TaskStorage::new(inner),
            parent_task_enrichment,
            sink,
            result_tx: Some(result_tx),
            shutdown_signal: None,
            telemetry: None,
        }
    }

    pub(super) fn new_with_shutdown(
        inner: F,
        result_tx: BoxedSender<TaskResult<R>>,
        parent_task_enrichment: Transfer,
        sink: Sink,
        shutdown_signal: Option<Arc<AtomicBool>>,
        telemetry: TaskTelemetry,
    ) -> Self {
        Self {
            inner: TaskStorage::new(inner),
            parent_task_enrichment,
            sink,
            result_tx: Some(result_tx),
            shutdown_signal,
            telemetry: Some(telemetry),
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
        let result_tx = this.result_tx.take();
        if this.inner.is_live() {
            let _guard = this.parent_task_enrichment.apply_current_thread();
            match this.inner.destroy_pinned() {
                Ok(()) => {
                    if let Some(telemetry) = this.telemetry.as_mut() {
                        telemetry.canceled();
                    }
                }
                Err(panic) => {
                    if let Some(telemetry) = this.telemetry.as_mut() {
                        telemetry.panicked();
                    }
                    emit!(this.sink, TaskPanicked);
                    discard_panic(panic);
                }
            }
        }
        if let Some(sender) = result_tx {
            dispose_sender(|| drop(sender));
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
        let mut this = self.project();
        let _guard = this.parent_task_enrichment.apply_current_thread();

        if this.shutdown_signal.as_ref().is_some_and(|signal| signal.load(Ordering::Acquire)) {
            if this.inner.is_live() {
                match this.inner.as_mut().destroy_pinned() {
                    Ok(()) => {
                        if let Some(telemetry) = this.telemetry.as_mut() {
                            telemetry.canceled();
                        }
                    }
                    Err(panic) => {
                        if let Some(telemetry) = this.telemetry.as_mut() {
                            telemetry.panicked();
                        }
                        emit!(this.sink, TaskPanicked);
                        discard_panic(panic);
                    }
                }
            }
            let _ = this.result_tx.take().map(|sender| dispose_sender(|| drop(sender)));
            return Poll::Ready(());
        }

        let poll_telemetry = this.telemetry.as_ref().map(|telemetry| telemetry.poll_started());
        // We AssertUnwindSafe here because we consider the task completed on panic, which means
        // it will never be polled again - whatever it did to its internal state is now
        // irrelevant and if it corrupted some shared state, that is not really something we
        // can do anything about (a conscientious service will abort on panic to avoid that).
        let inner_poll_result = catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx)));
        drop(poll_telemetry);

        match inner_poll_result {
            Ok(Poll::Ready(result)) => {
                // An abandoned join destroys the result here, on the task's worker.
                let sender = this.result_tx.take().expect("future polled after completion");
                if let Some(telemetry) = this.telemetry.as_mut() {
                    telemetry.completed();
                }
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
                if let Err(disposal) = this.inner.as_mut().destroy_pinned() {
                    discard_panic(disposal);
                }
                if let Some(telemetry) = this.telemetry.as_mut() {
                    telemetry.panicked();
                }
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::cell::Cell;
    use std::pin::pin;
    use std::sync::Arc as StdArc;
    use std::task::{Context, Wake, Waker};

    use events_once::Event;

    use super::*;
    use crate::runtime::seismograph::RuntimeTelemetry;

    struct PanicOnPoll {
        dropped: StdArc<AtomicBool>,
    }

    impl Future for PanicOnPoll {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            panic!("task panic");
        }
    }

    impl Drop for PanicOnPoll {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    struct PanicOnPollAndDrop {
        dropped: StdArc<AtomicBool>,
    }

    impl Future for PanicOnPollAndDrop {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            panic!("task panic");
        }
    }

    impl Drop for PanicOnPollAndDrop {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
            panic!("task destructor panic");
        }
    }

    struct AssertDroppedOnWake {
        dropped: StdArc<AtomicBool>,
    }

    impl Wake for AssertDroppedOnWake {
        fn wake(self: StdArc<Self>) {
            assert!(self.dropped.load(Ordering::Acquire));
        }
    }

    #[test]
    fn sender_disposal_panic_is_contained() {
        let invoked = Cell::new(false);
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            dispose_sender(|| {
                invoked.set(true);
                panic!("sender drop");
            });
        }))
        .unwrap();
        assert!(invoked.get());
    }

    #[test]
    fn shutdown_signal_discards_a_direct_task_result() {
        let sink = Sink::noop();
        let (sender, _receiver) = Event::<TaskResult<u32>>::boxed();
        let signal = Arc::new(AtomicBool::new(true));
        let (runtime_telemetry, _workers) = RuntimeTelemetry::register(0..1, Sink::noop());
        let task_telemetry = runtime_telemetry.register_task::<std::future::Ready<u32>>(0);
        let task_telemetry = task_telemetry.materialized();
        let mut task = pin!(RemoteTaskFuture::new_with_shutdown(
            std::future::ready(42),
            sender,
            sink.transfer_context(),
            sink,
            Some(signal),
            task_telemetry,
        ));

        assert_eq!(task.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));
    }

    #[test]
    fn panic_notification_follows_future_retirement() {
        let sink = Sink::noop();
        let dropped = StdArc::new(AtomicBool::new(false));
        let (sender, receiver) = Event::<TaskResult<()>>::boxed();
        let mut receiver = pin!(receiver);
        let waker = Waker::from(StdArc::new(AssertDroppedOnWake {
            dropped: StdArc::clone(&dropped),
        }));
        assert!(receiver.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

        let mut task = pin!(RemoteTaskFuture::new(
            PanicOnPoll {
                dropped: StdArc::clone(&dropped),
            },
            sender,
            sink.transfer_context(),
            sink,
        ));
        assert_eq!(task.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));
        assert!(dropped.load(Ordering::Acquire));
        assert!(matches!(
            receiver.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(TaskResult::Panicked(_)))
        ));
    }

    #[test]
    fn poll_and_destructor_panics_are_contained_before_notification() {
        let sink = Sink::noop();
        let dropped = StdArc::new(AtomicBool::new(false));
        let (sender, receiver) = Event::<TaskResult<()>>::boxed();
        let mut task = pin!(RemoteTaskFuture::new(
            PanicOnPollAndDrop {
                dropped: StdArc::clone(&dropped),
            },
            sender,
            sink.transfer_context(),
            sink,
        ));

        assert_eq!(task.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));
        assert!(dropped.load(Ordering::Acquire));
        assert!(matches!(futures::executor::block_on(receiver).unwrap(), TaskResult::Panicked(_)));
    }
}
