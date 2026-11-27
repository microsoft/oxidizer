// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Context passed to [`Actor::handle`](crate::Actor::handle).
//!
//! Every running actor receives a [`Context`] while handling each message. Use
//! it to get this actor's [`ActorRef`], spawn child actors on this actor's
//! worker, or request stop or drain after the current message.

use super::{ActorId, ActorRef};
use crate::inbox::{Control, Signal};
use crate::{ActorRuntime, Inbox, Message};

/// Context passed to [`Actor::handle`](crate::Actor::handle).
#[derive(Debug, Clone)]
pub struct Context<M: Message> {
    actor_ref: ActorRef<M>,
    runtime: ActorRuntime,
    control: Option<Control>,
}

impl<M: Message> Context<M> {
    /// Initializes the context for a running actor.
    pub(crate) const fn new(actor_ref: ActorRef<M>, runtime: ActorRuntime) -> Self {
        Self {
            actor_ref,
            runtime,
            control: None,
        }
    }

    /// Borrows this actor's [`ActorRef`].
    ///
    /// The returned handle targets this actor. Clone it when a task needs an
    /// owned handle that can outlive the current `handle` call.
    #[must_use]
    pub const fn actor_ref(&self) -> &ActorRef<M> {
        &self.actor_ref
    }

    /// Returns the id of this actor.
    #[must_use]
    pub const fn id(&self) -> ActorId {
        self.actor_ref.id()
    }

    /// Borrows a runtime that spawns onto this actor's worker.
    ///
    /// A spawn with no placement, such as [`ActorRuntime::spawn`], runs the new
    /// actor on the same worker as this actor. Other placements work as they do
    /// on any [`ActorRuntime`].
    #[must_use]
    pub const fn runtime(&self) -> &ActorRuntime {
        &self.runtime
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
