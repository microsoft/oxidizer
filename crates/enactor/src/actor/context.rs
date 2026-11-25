// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Context passed to [`Actor::handle`](crate::Actor::handle).
//!
//! Every running actor receives a [`Context`] while handling each message. Use
//! it to get this actor's [`ActorRef`] or request stop or drain after the
//! current message.

use super::ActorRef;
use crate::inbox::{Control, Signal};
use crate::{Inbox, Message};

/// Context passed to [`Actor::handle`](crate::Actor::handle).
#[derive(Debug, Clone)]
pub struct Context<M: Message> {
    actor_ref: ActorRef<M>,
    control: Option<Control>,
}

impl<M: Message> Context<M> {
    /// Initializes the context for a running actor.
    pub(crate) const fn new(actor_ref: ActorRef<M>) -> Self {
        Self { actor_ref, control: None }
    }

    /// Borrows this actor's [`ActorRef`].
    ///
    /// The returned handle targets this actor. Clone it when a task needs an
    /// owned handle that can outlive the current `handle` call.
    #[must_use]
    pub const fn actor_ref(&self) -> &ActorRef<M> {
        &self.actor_ref
    }

    /// Stops the actor after the current message finishes.
    pub const fn stop(&mut self) {
        self.control = Some(Control::Signal(Signal::Stop));
    }

    /// Delivers queued messages after the current one, then stops the actor.
    pub fn drain(&mut self) {
        if self.control != Some(Control::Signal(Signal::Stop)) {
            self.control = Some(Control::Signal(Signal::Drain));
        }
    }

    /// Applies any pending lifecycle request to `inbox`.
    ///
    /// The return value is `true` when the actor loop should exit after the current message.
    pub(crate) fn apply(&mut self, inbox: &Inbox<M>) -> bool {
        let Some(control) = self.control.take() else {
            return false;
        };

        match control {
            Control::Signal(Signal::Stop) => {
                inbox.stop();
                true
            }
            Control::Signal(Signal::Drain) => {
                inbox.drain();
                false
            }
            Control::Command(command) => match command {},
        }
    }
}
