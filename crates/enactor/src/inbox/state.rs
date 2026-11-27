// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Inbox delivery state machine.

use super::control::Control;

/// Delivery state for one inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Running,
    Draining,
    Stopped,
}

impl State {
    pub(super) const fn transition(self, control: Control) -> Self {
        match control {
            Control::Signal(signal) => signal.apply(self),
            Control::Command(command) => command.apply(self),
        }
    }
}
