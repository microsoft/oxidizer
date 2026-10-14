// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Mutex;
use std::task;

use events_once::{BoxedReceiver, BoxedSender, LocalEventLake, PooledLocalReceiver, PooledLocalSender};

use crate::TaskPanicPayload;

enum TaskResult<R> {
    Completed(R),
    Panicked(TaskPanicPayload),
}

impl<R> Debug for TaskResult<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed(_) => f.write_str("Completed(..)"),
            Self::Panicked(_) => f.write_str("Panicked(..)"),
        }
    }
}

/// A task failed to return its result.
pub struct JoinError {
    panic: Option<Mutex<TaskPanicPayload>>,
}

impl PartialEq for JoinError {
    fn eq(&self, other: &Self) -> bool {
        self.is_panic() == other.is_panic()
    }
}

impl Eq for JoinError {}

impl JoinError {
    #[must_use]
    const fn shutdown() -> Self {
        Self { panic: None }
    }

    pub(crate) fn panicked(panic: TaskPanicPayload) -> Self {
        Self {
            panic: Some(Mutex::new(panic)),
        }
    }

    /// Returns `true` if the task panicked.
    #[must_use]
    pub const fn is_panic(&self) -> bool {
        self.panic.is_some()
    }

    /// Returns `true` if executor shutdown cancelled the task or rejected its submission.
    #[must_use]
    pub const fn is_shutdown(&self) -> bool {
        self.panic.is_none()
    }

    /// Returns the captured panic payload.
    ///
    /// # Panics
    ///
    /// Panics if this error represents executor shutdown rather than a task panic.
    #[must_use]
    pub fn into_panic(self) -> TaskPanicPayload {
        self.panic
            .expect("into_panic() requires a JoinError for which is_panic() is true")
            .into_inner()
            .expect("the private panic payload mutex cannot be poisoned")
    }
}

impl Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_panic() {
            "task panicked"
        } else {
            "runtime is shutting down"
        })
    }
}

impl Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("is_panic", &self.is_panic())
            .field("is_shutdown", &self.is_shutdown())
            .finish()
    }
}

impl Error for JoinError {}

/// Owns pooled result storage whose detached slots remain valid through their endpoints.
#[derive(Clone, Debug)]
pub(crate) struct JoinEventLake {
    events: LocalEventLake,
}

impl JoinEventLake {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            events: LocalEventLake::new(),
        }
    }

    #[must_use]
    pub(crate) fn rent<R: 'static>(&self) -> (PooledLocalSender<R>, JoinHandle<R>) {
        let (tx, rx) = self.events.rent();
        (tx, JoinHandle::new(rx))
    }

    #[must_use]
    pub(crate) fn disconnected<R: 'static>(&self) -> JoinHandle<R> {
        let (tx, handle) = self.rent();
        drop(tx);
        handle
    }
}

/// A handle that can be used to await the completion of a task and to receive its result `R`.
///
/// Completion produces `Ok(result)`. If executor shutdown cancels the task or rejects its
/// submission, awaiting the handle produces [`JoinError`].
///
/// A join handle may outlive its executor. Dropping the handle does not cancel the task.
///
/// # Panics
///
/// Panics if polled again after returning `Ready`.
#[derive(Debug)]
pub struct JoinHandle<R: 'static> {
    rx: PooledLocalReceiver<R>,
}

impl<R: 'static> JoinHandle<R> {
    fn new(rx: PooledLocalReceiver<R>) -> Self {
        Self { rx }
    }
}

impl<R: 'static> Future for JoinHandle<R> {
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        Pin::new(&mut self.get_mut().rx)
            .poll(cx)
            .map(|result| result.map_err(|_disconnected| JoinError::shutdown()))
    }
}

/// A cross-thread handle for receiving a task result.
#[doc(hidden)]
#[derive(Debug)]
pub struct RemoteJoinHandle<R: Send + 'static> {
    rx: BoxedReceiver<TaskResult<R>>,
}

impl<R: Send + 'static> RemoteJoinHandle<R> {
    /// Creates a result sender and its matching join handle.
    #[doc(hidden)]
    #[must_use]
    pub fn channel() -> (RemoteTaskSender<R>, Self) {
        let (tx, rx) = events_once::Event::boxed();
        (RemoteTaskSender { tx }, Self { rx })
    }
}

impl<R: Send + 'static> Future for RemoteJoinHandle<R> {
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        Pin::new(&mut self.get_mut().rx).poll(cx).map(|result| match result {
            Ok(TaskResult::Completed(value)) => Ok(value),
            Ok(TaskResult::Panicked(panic)) => Err(JoinError::panicked(panic)),
            Err(_) => Err(JoinError::shutdown()),
        })
    }
}

/// The sending endpoint for a [`RemoteJoinHandle`].
#[doc(hidden)]
pub struct RemoteTaskSender<R: Send + 'static> {
    tx: BoxedSender<TaskResult<R>>,
}

impl<R: Send + 'static> Debug for RemoteTaskSender<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteTaskSender").field("tx", &self.tx).finish()
    }
}

impl<R: Send + 'static> RemoteTaskSender<R> {
    fn send(self, result: TaskResult<R>) -> Result<(), TaskPanicPayload> {
        catch_unwind(AssertUnwindSafe(|| self.tx.send(result)))
    }

    /// Sends a successfully completed result.
    #[doc(hidden)]
    pub fn completed(self, result: R) -> Result<(), TaskPanicPayload> {
        self.send(TaskResult::Completed(result))
    }

    /// Sends a captured task panic.
    #[doc(hidden)]
    pub fn panicked(self, panic: TaskPanicPayload) -> Result<(), TaskPanicPayload> {
        self.send(TaskResult::Panicked(panic))
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // We expect these join handles to be single-threaded.
    assert_not_impl_any!(JoinHandle<u8>: Send, Sync);

    // We do not expect there to be anything that requires pinning the join handle.
    assert_impl_all!(JoinHandle<u8>: Unpin);

    #[test]
    fn shutdown_error_contract() {
        assert_impl_all!(JoinError: Error, Send, Sync);

        let error = JoinError::shutdown();

        assert!(error.is_shutdown());
        assert!(!error.is_panic());
        assert_eq!(error.to_string(), "runtime is shutting down");
        assert!(format!("{error:?}").contains("is_shutdown: true"));
        assert!(error.source().is_none());
    }

    #[test]
    fn panic_error_exposes_its_payload() {
        let error = JoinError::panicked(Box::new(42_u64));

        assert!(error.is_panic());
        assert!(!error.is_shutdown());
        assert_eq!(*error.into_panic().downcast::<u64>().unwrap(), 42);
    }
}
