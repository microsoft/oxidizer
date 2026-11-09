// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

use super::registry::{PanicAction, TypedHook};

/// Reports that Arty contained a panic from user task code.
///
/// This event covers async task factories, async task polls, and blocking
/// functions. An async event runs on the task's worker before Arty notifies its
/// join handle. A blocking function runs on a pool thread, so Arty sends its
/// event to the associated async worker before it notifies the join handle.
///
/// The default response is shutdown. Call [`ignore`](Self::ignore) from the
/// callback to keep the runtime available. Call [`shutdown`](Self::shutdown) to
/// select shutdown explicitly. The failed task returns
/// [`JoinError`](crate::task::JoinError) with either response.
///
/// The callback must not panic. Arty cannot repair application data that a
/// task panic left in an invalid state.
#[derive(derive_more::Debug)]
pub struct OnPanic {
    pub(super) action: PanicAction,
}

impl OnPanic {
    pub(crate) fn new() -> Self {
        Self {
            action: PanicAction::Shutdown,
        }
    }

    pub(crate) fn action(&self) -> PanicAction {
        self.action
    }

    /// Keeps the runtime running after this contained panic.
    pub fn ignore(&mut self) {
        self.action = PanicAction::Ignore;
    }

    /// Stops the runtime after this contained panic.
    pub fn shutdown(&mut self) {
        self.action = PanicAction::Shutdown;
    }
}

#[derive(Clone, derive_more::Debug)]
pub(crate) struct PanicHook(TypedHook<OnPanic>);

impl PanicHook {
    pub(crate) fn new<D>(data: D, callback: fn(&mut OnPanic, &mut D)) -> Self
    where
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        Self(TypedHook::new(data, callback))
    }
    pub(crate) fn execute(&mut self, event: &mut OnPanic) {
        self.0.execute(event);
    }
}

impl thread_aware::ThreadAware for PanicHook {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.0.relocate(source, destination);
    }
}
