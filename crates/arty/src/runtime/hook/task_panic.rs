// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{PanicAction, TaskPanicPayload};

use super::hook_fn::HookFn;

/// Reports that Arty contained a panic from user task code.
///
/// This event covers async task factories and async task polls. It runs on the
/// task's worker before Arty notifies its join handle. A blocking function
/// panic returns through its join handle without creating this event or
/// stopping the runtime.
///
/// Each event starts with shutdown selected. The worker runs its task-panic
/// hooks on the same event in the order it installed them, then applies the
/// final response. A callback that only reads the event leaves the response
/// unchanged.
///
/// Register [`TaskPanic::ignore`] to keep the runtime running. A later hook can
/// select shutdown again. Custom callbacks can use [`set_ignore`](Self::set_ignore)
/// or [`set_shutdown`](Self::set_shutdown) to choose a response for this panic.
/// The failed task returns [`JoinError`](crate::task::JoinError) with either
/// response.
///
/// Arty does not catch a panic from the callback. You must make sure that the
/// callback completes without a panic. Arty also cannot repair application
/// data that the original task panic left in an invalid state.
#[derive(derive_more::Debug)]
pub struct TaskPanic {
    pub(super) action: PanicAction,
    #[debug(ignore)]
    pub(super) payload: TaskPanicPayload,
}

impl TaskPanic {
    /// Creates a hook that selects runtime shutdown for each contained task panic.
    ///
    /// Shutdown is the default. Register this hook to override an earlier ignore response.
    #[must_use]
    pub const fn shutdown() -> HookFn<Self, ()> {
        HookFn::new((), |event, ()| event.set_shutdown())
    }

    /// Creates a hook that asks Arty to keep running after a contained task panic.
    #[must_use]
    pub const fn ignore() -> HookFn<Self, ()> {
        HookFn::new((), |event, ()| event.set_ignore())
    }

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

    /// Selects continued operation for this panic.
    pub fn set_ignore(&mut self) {
        self.action = PanicAction::Ignore;
    }

    /// Selects runtime shutdown for this panic.
    pub fn set_shutdown(&mut self) {
        self.action = PanicAction::Shutdown;
    }
}

impl super::Hook for TaskPanic {}

impl super::private::Sealed for TaskPanic {
    type Erased<D>
        = super::hook_fn::ErasedHookFn<Self>
    where
        D: thread_aware::ThreadAware + Clone + Sync + 'static;
}
