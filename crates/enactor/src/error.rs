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

/// Error returned by [`ActorRef::ask`](crate::ActorRef::ask) and [`ask!`](macro@crate::ask).
#[ohno::error]
#[from(TellError)]
#[from(CanceledError)]
pub struct AskError;

impl AskError {
    /// Returns `true` if the error was caused by the actor dropping the reply
    /// channel before sending a response.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.source().and_then(|s| s.downcast_ref::<CanceledError>()).is_some()
    }
}

impl From<oneshot::RecvError> for AskError {
    #[track_caller]
    fn from(error: oneshot::RecvError) -> Self {
        Self::from(CanceledError::from(error))
    }
}
