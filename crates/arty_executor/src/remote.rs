// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pooled cross-thread task result delivery.
//!
//! Sending a task result back to a different thread is a common runtime integration scenario.
//! Store a [`Pool`] in the adopting component and call [`Pool::channel`] for each task. Completed
//! channels return their storage to the pool after both endpoints are dropped, avoiding a fresh
//! channel allocation when reusable capacity is available.

use std::fmt::{self, Debug};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task;

use events_once::{EventLake, PooledReceiver, PooledSender};

use crate::{JoinError, ShutdownCause, TaskPanicPayload, discard_task_panic_payload};

enum TaskResult<R> {
    Completed(R),
    Panicked(TaskPanicPayload),
    Shutdown(ShutdownCause),
}

impl<R> Debug for TaskResult<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed(_) => f.write_str("Completed(..)"),
            Self::Panicked(_) => f.write_str("Panicked(..)"),
            Self::Shutdown(_) => f.write_str("Shutdown(..)"),
        }
    }
}

/// Reusable storage for cross-thread task result channels.
///
/// Clones share the same storage pool. A channel slot becomes reusable after both its [`Sender`]
/// and [`JoinHandle`] are dropped.
#[derive(Clone)]
pub struct Pool {
    events: EventLake,
}

impl Pool {
    /// Creates an empty cross-thread result channel pool.
    #[must_use]
    pub fn new() -> Self {
        Self { events: EventLake::new() }
    }

    /// Creates a sender and join handle backed by this pool.
    ///
    /// The sender and handle may be moved to different threads. Dropping the sender without
    /// publishing a result makes the handle resolve to a shutdown error.
    #[must_use]
    pub fn channel<R: Send + 'static>(&self) -> (Sender<R>, JoinHandle<R>) {
        let (tx, rx) = self.events.rent();
        (
            Sender {
                tx: Some(tx),
                panic_notifier: None,
            },
            JoinHandle { rx },
        )
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool").finish_non_exhaustive()
    }
}

/// A cross-thread handle for receiving a task result.
pub struct JoinHandle<R: Send + 'static> {
    rx: PooledReceiver<TaskResult<R>>,
}

impl<R: Send + 'static> Future for JoinHandle<R> {
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        Pin::new(&mut self.get_mut().rx).poll(cx).map(|result| match result {
            Ok(TaskResult::Completed(value)) => Ok(value),
            Ok(TaskResult::Panicked(panic)) => Err(JoinError::panicked(panic)),
            Ok(TaskResult::Shutdown(cause)) => Err(JoinError::shutdown(cause)),
            Err(_) => Err(JoinError::shutdown(ShutdownCause::default())),
        })
    }
}

impl<R: Send + 'static> Debug for JoinHandle<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}

/// The sending endpoint for a [`JoinHandle`].
pub struct Sender<R: Send + 'static> {
    tx: Option<PooledSender<TaskResult<R>>>,
    panic_notifier: Option<task::Waker>,
}

impl<R: Send + 'static> Sender<R> {
    fn send(&mut self, result: TaskResult<R>) -> Result<(), TaskPanicPayload> {
        let tx = self.tx.take().expect("a remote task sender publishes at most one result");
        catch_unwind(AssertUnwindSafe(|| tx.send(result)))
    }

    /// Configures a runtime-integration notification for a captured task panic.
    ///
    /// The sender wakes the notifier after publishing the panic result.
    /// Applications should use `arty` instead of this integration API.
    #[doc(hidden)]
    #[must_use]
    pub fn with_panic_notifier(mut self, notifier: task::Waker) -> Self {
        self.panic_notifier = Some(notifier);
        self
    }

    /// Sends a successfully completed result.
    ///
    /// # Errors
    ///
    /// Returns a panic payload if notifying the result receiver panics.
    pub fn completed(mut self, result: R) -> Result<(), TaskPanicPayload> {
        self.send(TaskResult::Completed(result))
    }

    /// Sends a captured task panic.
    ///
    /// # Errors
    ///
    /// Returns a panic payload if notifying the result receiver panics.
    pub fn panicked(mut self, panic: TaskPanicPayload) -> Result<(), TaskPanicPayload> {
        let delivery = self.send(TaskResult::Panicked(panic));
        if let Some(notifier) = self.panic_notifier.take() {
            notifier.wake();
        }
        delivery
    }

    /// Sends executor shutdown as the task outcome.
    ///
    /// # Errors
    ///
    /// Returns a panic payload if notifying the result receiver panics.
    pub fn shutdown(mut self, cause: ShutdownCause) -> Result<(), TaskPanicPayload> {
        self.send(TaskResult::Shutdown(cause))
    }
}

impl<R: Send + 'static> Debug for Sender<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl<R: Send + 'static> Drop for Sender<R> {
    fn drop(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(tx))) {
            discard_task_panic_payload(panic);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(Pool: Clone, Debug, Default, Send, Sync);
    assert_impl_all!(JoinHandle<u8>: Debug, Send, Unpin);
    assert_not_impl_any!(JoinHandle<u8>: Sync);
    assert_impl_all!(Sender<u8>: Debug, Send, Unpin);
    assert_not_impl_any!(Sender<u8>: Sync);

    struct PanicNotifier(AtomicBool);

    impl Wake for PanicNotifier {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Release);
        }
    }

    fn poll_ready<R: Send + 'static>(handle: JoinHandle<R>) -> Result<R, JoinError> {
        let mut handle = pin!(handle);
        match handle.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("completed channel must be immediately ready"),
        }
    }

    #[test]
    fn channel_delivers_completed_result() {
        let pool = Pool::new();
        let (sender, handle) = pool.channel();

        sender.completed(42).unwrap();

        assert_eq!(poll_ready(handle), Ok(42));
    }

    #[test]
    fn disconnected_sender_reports_shutdown() {
        let pool = Pool::new();
        let (sender, handle) = pool.channel::<u8>();

        drop(sender);

        assert!(matches!(
            poll_ready(handle),
            Err(JoinError::Shutdown(cause)) if cause.as_error().is_none()
        ));
    }

    #[test]
    fn panic_result_notifies_runtime_integration() {
        let pool = Pool::new();
        let notifier = Arc::new(PanicNotifier(AtomicBool::new(false)));
        let (sender, handle) = pool.channel::<u8>();
        let sender = sender.with_panic_notifier(Waker::from(Arc::clone(&notifier)));

        sender.panicked(Box::new("task panic")).unwrap();

        assert!(notifier.0.load(Ordering::Acquire));
        assert!(matches!(poll_ready(handle), Err(JoinError::Panicked(_))));
    }

    #[test]
    fn diagnostics_cover_each_endpoint_and_result_kind() {
        let pool = Pool::default();
        let (sender, handle) = pool.channel::<u8>();

        assert_eq!(format!("{:?}", TaskResult::Completed(1)), "Completed(..)");
        assert_eq!(format!("{:?}", TaskResult::<u8>::Panicked(Box::new("panic"))), "Panicked(..)");
        assert_eq!(
            format!("{:?}", TaskResult::<u8>::Shutdown(ShutdownCause::default())),
            "Shutdown(..)"
        );
        assert!(format!("{pool:?}").starts_with("Pool"));
        assert!(format!("{sender:?}").starts_with(std::any::type_name::<Sender<u8>>()));
        assert!(format!("{handle:?}").starts_with("JoinHandle"));
    }
}
