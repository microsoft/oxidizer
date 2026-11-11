// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::TaskPanicPayload;
use thread_aware::Thread;

use super::registry::PanicAction;
use super::typed::{HookCallback, TypedHook};

/// Reports that Arty contained a panic from user task code.
///
/// This event covers async task factories and async task polls. It runs on the
/// task's worker before Arty notifies its join handle. A blocking function
/// panic returns through its join handle without creating this event or
/// stopping the runtime.
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
    #[debug(ignore)]
    pub(super) payload: TaskPanicPayload,
}

impl OnPanic {
    /// Keeps the runtime running after a contained panic.
    pub const IGNORE: fn(&mut Self, &()) = |event, ()| event.ignore();

    pub(crate) const fn new(payload: TaskPanicPayload) -> Self {
        Self {
            action: PanicAction::Shutdown,
            payload,
        }
    }

    pub(crate) fn into_parts(self) -> (PanicAction, TaskPanicPayload) {
        (self.action, self.payload)
    }

    /// Returns the contained panic payload.
    #[must_use]
    pub fn payload(&self) -> &(dyn std::any::Any + Send + 'static) {
        self.payload.as_ref()
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

impl super::Hook for OnPanic {}

#[derive(Clone, derive_more::Debug)]
pub(crate) struct PanicHook(TypedHook<OnPanic>);

impl PanicHook {
    pub(crate) fn new<D>(data: D, callback: HookCallback<OnPanic, D>) -> Self
    where
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        Self(TypedHook::<OnPanic>::new(data, callback))
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
