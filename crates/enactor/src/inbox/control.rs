// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Control-queue messages for actor inboxes.
//!
//! Stop and drain requests use a dedicated queue so they can preempt ordinary
//! message delivery.

use super::channel::{SendError, Sender};
use super::state::State;

/// Lifecycle signals sent over the inbox control queue.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    /// Drain queued messages, then stop.
    Drain = 1,
    /// Stop immediately.
    Stop = 2,
}

impl Signal {
    pub(super) const fn apply(self, state: State) -> State {
        match (state, self) {
            (State::Stopped, _) | (_, Self::Stop) => State::Stopped,
            _ => State::Draining,
        }
    }
}

/// Reserved for future inbox-local commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {}

impl Command {
    pub(super) const fn apply(self, _state: State) -> State {
        match self {}
    }
}

/// Item carried on the inbox control queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Signal(Signal),
    #[expect(dead_code, reason = "reserved for future inbox-consumed control requests")]
    Command(Command),
}

impl Sender<Control> {
    pub(crate) async fn stop(&self) -> Result<(), SendError<Control>> {
        self.send(Control::Signal(Signal::Stop)).await
    }

    pub(crate) async fn drain(&self) -> Result<(), SendError<Control>> {
        self.send(Control::Signal(Signal::Drain)).await
    }
}
