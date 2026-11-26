// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Errors returned by [`ActorRef::tell`](crate::ActorRef::tell) and
//! [`ActorRef::ask`](crate::ActorRef::ask), and task completion waits.

use std::error::Error;
use std::time::Duration;

use crate::inbox::SendError as ChannelSendError;

/// A task did not finish within the configured completion-wait timeout.
#[ohno::error]
#[display("completion wait timed out after {timeout:?}")]
pub struct CompletionTimeoutError {
    /// The duration allowed for this wait, not for task execution.
    pub timeout: Duration,
}

impl CompletionTimeoutError {
    /// Creates an error for a wait that exhausted `timeout`.
    #[must_use]
    pub fn after(timeout: Duration) -> Self {
        Self::new(timeout)
    }
}

/// Error returned when a message could not be sent to an actor.
#[ohno::error]
#[display("failed to send message to actor")]
pub struct TellError;

impl<T> From<ChannelSendError<T>> for TellError {
    #[track_caller]
    fn from(_: ChannelSendError<T>) -> Self {
        // `ChannelSendError` cannot not be captured because T is not guaranteed to be Sync
        // There is not much value in capturing it anyway, since it only indicates that the channel
        // is closed.
        Self::new()
    }
}

/// The actor dropped the reply channel before sending a response.
#[ohno::error]
#[from(oneshot::RecvError)]
#[display("actor dropped the reply channel before sending a response")]
pub struct CanceledError;

/// The actor did not reply before the deadline of a timed ask.
#[ohno::error]
#[display("actor did not reply within {timeout:?}")]
pub struct AskTimeoutError {
    timeout: Duration,
}

/// Error returned by asks.
///
/// [`ActorRef::ask`](crate::ActorRef::ask), [`ask!`](macro@crate::ask), and
/// their timed forms such as [`ActorRef::ask_timeout`](crate::ActorRef::ask_timeout)
/// and [`ask_t!`](macro@crate::ask_t) return this error.
#[ohno::error]
#[from(TellError)]
#[from(CanceledError)]
#[from(AskTimeoutError)]
pub struct AskError;

impl AskError {
    /// Returns `true` if the error was caused by the actor dropping the reply
    /// channel before sending a response.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.source().and_then(|s| s.downcast_ref::<CanceledError>()).is_some()
    }

    /// Returns `true` if the deadline of a timed ask passed before the reply arrived.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        self.source().and_then(|s| s.downcast_ref::<AskTimeoutError>()).is_some()
    }
}

impl From<oneshot::RecvError> for AskError {
    #[track_caller]
    fn from(error: oneshot::RecvError) -> Self {
        Self::from(CanceledError::from(error))
    }
}
