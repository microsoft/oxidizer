// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::pin::Pin;
use std::task;

use events_once::{LocalEventLake, PooledLocalReceiver, PooledLocalSender};

/// A task failed to return its result because its executor shut down.
///
/// `arty_executor` does not recover task panics. A task panic that reaches this layer
/// terminates the process, so shutdown is the only recoverable join failure reported here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JoinError {
    _private: (),
}

impl JoinError {
    #[must_use]
    const fn shutdown() -> Self {
        Self { _private: () }
    }

    /// Returns `true` because executor shutdown is the only recoverable join failure.
    #[must_use]
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        clippy::unused_self,
        reason = "predicate methods conventionally inspect an error value and leave room for future failure details"
    )]
    pub const fn is_shutdown(&self) -> bool {
        true
    }
}

impl Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("task did not return a result because the executor is shut down")
    }
}

impl Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
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
        assert_eq!(error.to_string(), "task did not return a result because the executor is shut down");
        assert!(format!("{error:?}").contains("is_shutdown: true"));
        assert!(error.source().is_none());
    }
}
