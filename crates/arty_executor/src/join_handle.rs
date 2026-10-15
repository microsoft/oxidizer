// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::pin::Pin;
use std::sync::Mutex;
use std::task;

use events_once::{LocalEventLake, PooledLocalReceiver, PooledLocalSender};

use crate::TaskPanicPayload;

/// A task failed to return its result.
///
/// This error intentionally stores the classification and optional panic payload directly.
/// Shutdown has no underlying cause, and a panic payload need not implement [`Error`], so an error
/// source chain would not preserve additional typed diagnostic information.
pub struct JoinError {
    // Panic payloads are `Send` but need not be `Sync`. The mutex lets `JoinError` remain a
    // thread-safe error while preserving the original payload for `into_panic`.
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
    pub(crate) const fn shutdown() -> Self {
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

    /// Returns the captured panic payload, or `None` if this error represents shutdown.
    #[must_use]
    pub fn into_panic(self) -> Option<TaskPanicPayload> {
        self.panic.map(|panic| {
            // Consuming the mutex gives us sole ownership. Poison records a previous unwind; it
            // does not invalidate the payload, so returning that payload remains correct.
            panic.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner())
        })
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // We expect these join handles to be single-threaded.
    assert_not_impl_any!(JoinHandle<u8>: Send, Sync);

    // We do not expect there to be anything that requires pinning the join handle.
    assert_impl_all!(JoinHandle<u8>: Unpin);

    #[test]
    fn shutdown_error_contract() {
        assert_impl_all!(JoinError: Error, Send, Sync, UnwindSafe, RefUnwindSafe);

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
        assert_eq!(error.to_string(), "task panicked");
        assert_eq!(*error.into_panic().unwrap().downcast::<u64>().unwrap(), 42);
    }

    #[test]
    fn join_errors_compare_by_failure_kind() {
        let shutdown = JoinError::shutdown();
        let another_shutdown = JoinError::shutdown();
        let panic = JoinError::panicked(Box::new("first panic"));
        let another_panic = JoinError::panicked(Box::new("different panic"));

        assert_eq!(shutdown, another_shutdown);
        assert_eq!(panic, another_panic);
        assert_ne!(shutdown, panic);
    }
}
