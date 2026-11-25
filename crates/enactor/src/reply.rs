// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reply handles for ask responses.
//!
//! Ask variants carry a [`Reply<T>`] as their final field, and the actor uses
//! it to send one value back to the waiting caller.

/// Reply handle for one ask response.
///
/// Actors receive a `Reply<T>` and use it to send a value back to callers
/// waiting on [`ActorRef::ask`](crate::ActorRef::ask) or
/// [`ask!`](macro@crate::ask).
#[derive(Debug)]
pub struct Reply<T>(oneshot::Sender<T>);

impl<T> Reply<T> {
    /// Builds the handle and receiver pair used by the caller.
    #[must_use]
    pub fn channel() -> (Self, oneshot::Receiver<T>) {
        let (tx, rx) = oneshot::channel();
        (Self(tx), rx)
    }

    /// Sends the reply value.
    ///
    /// # Errors
    ///
    /// Returns the value inside [`oneshot::SendError`] if the caller no longer
    /// waits for the reply.
    pub fn send(self, value: T) -> Result<(), oneshot::SendError<T>> {
        self.0.send(value)
    }
}
