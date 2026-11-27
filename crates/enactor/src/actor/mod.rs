// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Actor definitions.
//!
//! An actor owns its state and processes one message at a time with exclusive
//! access to that state. Implement [`Actor`] for behavior, then spawn the value
//! with [`ActorRuntime`](crate::ActorRuntime).

use crate::Message;

mod actor_id;
mod actor_ref;
mod context;

pub use self::actor_id::ActorId;
pub use self::actor_ref::{ActorRef, TryTellFailure};
pub use self::context::Context;

/// Trait implemented by actor types.
pub trait Actor: Send + 'static {
    /// Message type accepted by this actor.
    type Message: Message;

    /// Processes one message with exclusive access to the actor state.
    fn handle(&mut self, msg: Self::Message, ctx: &mut Context<Self::Message>) -> impl Future<Output = ()> + Send;
}
