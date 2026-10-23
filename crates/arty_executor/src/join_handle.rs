// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::task;

use events_once::{LocalEventLake, PooledLocalReceiver};

use crate::{ShutdownCause, ShutdownError, TaskPanicPayload};

/// A task failed to return its result.
///
/// A shutdown error is available through [`Error::source`] when shutdown started
/// because of an error. A panic payload need not implement [`Error`].
pub enum JoinError {
    /// The task panicked.
    ///
    // Panic payloads are `Send` but need not be `Sync`. The mutex lets `JoinError` remain a
    // thread-safe error while preserving the original payload for `into_panic`.
    Panicked(Mutex<TaskPanicPayload>),

    /// Executor shutdown cancelled the task or rejected its submission.
    Shutdown(Option<ShutdownError>),
}

impl PartialEq for JoinError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Panicked(_), Self::Panicked(_)) | (Self::Shutdown(_), Self::Shutdown(_))
        )
    }
}

impl Eq for JoinError {}

impl JoinError {
    #[must_use]
    pub(crate) fn shutdown(cause: ShutdownCause) -> Self {
        Self::Shutdown(cause.into_error())
    }

    pub(crate) fn panicked(panic: TaskPanicPayload) -> Self {
        Self::Panicked(Mutex::new(panic))
    }

}

impl Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Panicked(_) => "task panicked",
            Self::Shutdown(_) => "runtime is shutting down",
        })
    }
}

impl Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field(
                "kind",
                &match self {
                    Self::Panicked(_) => "panicked",
                    Self::Shutdown(_) => "shutdown",
                },
            )
            .finish_non_exhaustive()
    }
}

impl Error for JoinError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Shutdown(Some(error)) => Some(error),
            Self::Panicked(_) | Self::Shutdown(None) => None,
        }
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
    shutdown_cause: Rc<OnceLock<ShutdownCause>>,
}

impl<R: 'static> JoinHandle<R> {
    pub(crate) fn new(rx: PooledLocalReceiver<R>, shutdown_cause: Rc<OnceLock<ShutdownCause>>) -> Self {
        Self { rx, shutdown_cause }
    }

    pub(crate) fn disconnected(events: &LocalEventLake, shutdown_cause: Rc<OnceLock<ShutdownCause>>) -> Self {
        let (tx, rx) = events.rent();
        drop(tx);
        Self::new(rx, shutdown_cause)
    }
}

impl<R: 'static> Future for JoinHandle<R> {
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = self.get_mut();
        Pin::new(&mut this.rx).poll(cx).map(|result| {
            result.map_err(|_disconnected| {
                let cause = this.shutdown_cause.get().cloned().unwrap_or_default();
                JoinError::shutdown(cause)
            })
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // We expect these join handles to be single-threaded.
    assert_not_impl_any!(JoinHandle<u8>: Send, Sync);

    // We do not expect there to be anything that requires pinning the join handle.
    assert_impl_all!(JoinHandle<u8>: RefUnwindSafe, Unpin, UnwindSafe);

    #[test]
    fn shutdown_error_contract() {
        assert_impl_all!(JoinError: Error, Send, Sync, UnwindSafe, RefUnwindSafe);

        let error = JoinError::shutdown(ShutdownCause::default());

        assert!(matches!(error, JoinError::Shutdown(None)));
        assert_eq!(error.to_string(), "runtime is shutting down");
        assert!(format!("{error:?}").contains("shutdown"));
        assert!(error.source().is_none());
    }

    #[test]
    fn panic_error_exposes_its_payload() {
        let error = JoinError::panicked(Box::new(42_u64));

        assert_eq!(error.to_string(), "task panicked");
        assert!(matches!(&error, JoinError::Panicked(_)));
        let JoinError::Panicked(payload) = error else {
            return;
        };
        let payload = payload.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(*payload.downcast::<u64>().unwrap(), 42);
    }

    #[test]
    fn join_errors_compare_by_failure_kind() {
        let shutdown = JoinError::shutdown(ShutdownCause::default());
        let another_shutdown = JoinError::shutdown(ShutdownCause::default());
        let panic = JoinError::panicked(Box::new("first panic"));
        let another_panic = JoinError::panicked(Box::new("different panic"));

        assert_eq!(shutdown, another_shutdown);
        assert_eq!(panic, another_panic);
        assert_ne!(shutdown, panic);
    }
}
