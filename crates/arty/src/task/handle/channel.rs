// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::{ShutdownCause, TaskCompletion, TaskPanicPayload, discard_task_panic_payload};
use events_once::{EventLake, PooledSender};

use crate::runtime::Shutdown;
use crate::task::{JoinError, JoinHandle};

/// Creates result-channel endpoints that share one runtime shutdown state.
#[derive(Clone, Debug)]
pub(crate) struct JoinHandlePool {
    events: EventLake,
    shutdown: Shutdown,
}

impl JoinHandlePool {
    pub(crate) fn new(shutdown: Shutdown) -> Self {
        Self {
            events: EventLake::new(),
            shutdown,
        }
    }

    pub(crate) const fn shutdown(&self) -> &Shutdown {
        &self.shutdown
    }

    pub(crate) fn handle<R: Send + 'static>(&self) -> (JoinHandle<R>, Sender<R>) {
        let (tx, rx) = self.events.rent();
        (
            JoinHandle::new(rx, self.shutdown.clone()),
            Sender {
                tx: Some(tx),
                shutdown: self.shutdown.clone(),
            },
        )
    }
}

/// Adapts executor completion into Arty's result channel.
///
/// It publishes an executor shutdown cause before it notifies the joiner and
/// contains failures from result notification or sender disposal.
#[derive(derive_more::Debug)]
pub(crate) struct Sender<R: Send + 'static> {
    #[debug(ignore)]
    tx: Option<PooledSender<Result<R, JoinError>>>,
    shutdown: Shutdown,
}

impl<R: Send + 'static> Sender<R> {
    pub(crate) const fn shutdown(&self) -> &Shutdown {
        &self.shutdown
    }

    fn publish(mut self, result: Result<R, JoinError>) {
        let tx = self.tx.take().expect("an Arty result sender publishes at most one result");
        discard_unwind(|| tx.send(result));
    }
}

impl<R: Send + 'static> TaskCompletion<R> for Sender<R> {
    fn completed(self, result: R) {
        self.publish(Ok(result));
    }

    fn panicked(self, panic: TaskPanicPayload, cause: Option<ShutdownCause>) {
        if let Some(cause) = cause {
            self.shutdown.begin(cause);
        }
        self.publish(Err(JoinError::panicked(panic)));
    }

    fn cancelled(self, cause: ShutdownCause) {
        self.shutdown.begin(cause);
        let error = JoinError::shutdown(self.shutdown.error());
        self.publish(Err(error));
    }
}

impl<R: Send + 'static> Drop for Sender<R> {
    fn drop(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
        discard_unwind(|| drop(tx));
    }
}

fn discard_unwind(action: impl FnOnce()) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(action)) {
        discard_task_panic_payload(panic);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error as _;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    #[test]
    fn completion_delivers_the_result_to_the_handle() {
        let pool = JoinHandlePool::new(Shutdown::default());
        let (mut handle, sender) = pool.handle::<u32>();

        assert!(Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop())).is_pending());
        sender.completed(42);

        assert_eq!(handle.join().unwrap(), 42);
    }

    #[test]
    fn cancellation_reports_shutdown() {
        let pool = JoinHandlePool::new(Shutdown::default());
        let (mut handle, sender) = pool.handle::<u32>();

        sender.cancelled(ShutdownCause::default());

        assert!(matches!(
            Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(error)) if error.is_shutdown()
        ));
    }

    #[test]
    fn disconnected_sender_reports_shutdown() {
        let pool = JoinHandlePool::new(Shutdown::default());
        let (mut handle, sender) = pool.handle::<u32>();

        drop(sender);

        assert!(matches!(
            Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(error)) if error.is_shutdown()
        ));
    }

    #[test]
    fn cancellation_preserves_the_executor_shutdown_cause() {
        let pool = JoinHandlePool::new(Shutdown::default());
        let (handle, sender) = pool.handle::<u32>();
        let cause = ShutdownCause::panic();
        let expected = cause.as_error().unwrap().clone();

        sender.cancelled(cause);
        let error = handle.join().unwrap_err();

        assert!(error.is_shutdown());
        let source = error.source().unwrap().downcast_ref::<arty_executor::ShutdownError>().unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }

    #[test]
    fn cancellation_preserves_the_authoritative_runtime_shutdown_cause() {
        let shutdown = Shutdown::default();
        let pool = JoinHandlePool::new(shutdown.clone());
        let (handle, sender) = pool.handle::<u32>();
        let cause = ShutdownCause::panic();
        let expected = cause.as_error().unwrap().clone();
        shutdown.begin(cause);

        sender.cancelled(ShutdownCause::panic());
        let error = handle.join().unwrap_err();

        assert!(error.is_shutdown());
        let source = error.source().unwrap().downcast_ref::<arty_executor::ShutdownError>().unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }

    #[test]
    fn disconnection_preserves_the_runtime_shutdown_cause() {
        let shutdown = Shutdown::default();
        let pool = JoinHandlePool::new(shutdown.clone());
        let (handle, sender) = pool.handle::<u32>();
        let cause = ShutdownCause::panic();
        let expected = cause.as_error().unwrap().clone();

        shutdown.begin(cause);
        drop(sender);
        let error = handle.join().unwrap_err();

        assert!(error.is_shutdown());
        let source = error.source().unwrap().downcast_ref::<arty_executor::ShutdownError>().unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }

    #[test]
    fn receiving_a_panic_does_not_start_shutdown() {
        let shutdown = Shutdown::default();
        let pool = JoinHandlePool::new(shutdown.clone());
        let (handle, sender) = pool.handle::<u32>();

        sender.panicked(Box::new(42_u32), None);
        let error = handle.join().unwrap_err();

        assert_eq!(*error.into_panic().unwrap().downcast::<u32>().unwrap(), 42);
        assert_eq!(shutdown.state(), crate::runtime::ShutdownState::Running);
    }

    #[test]
    fn panic_preserves_the_executor_shutdown_cause_before_joining() {
        let shutdown = Shutdown::default();
        let pool = JoinHandlePool::new(shutdown.clone());
        let (handle, sender) = pool.handle::<u32>();
        let cause = ShutdownCause::panic();
        let expected = cause.as_error().unwrap().clone();

        sender.panicked(Box::new(42_u32), Some(cause));

        assert!(std::ptr::addr_eq(shutdown.error().unwrap().inner(), expected.inner()));
        assert_eq!(*handle.join().unwrap_err().into_panic().unwrap().downcast::<u32>().unwrap(), 42);
    }
}
