// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

use super::hook_fn::HookFn;
use super::registry::HookRegistration;

/// Reports that an async worker is starting.
///
/// Arty relocates the callback data to the worker before it creates this event.
/// The callback runs after the worker installs its scheduler and worker-local
/// services. It runs before the worker reports readiness, before
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns,
/// and before the runtime accepts tasks.
///
/// A late registration through
/// [`RuntimeControl::on`](crate::runtime::RuntimeControl::on) stores the
/// callback. It does not create an event because all workers have already
/// started.
#[derive(derive_more::Debug)]
pub struct OnStart {
    pub(super) thread: Thread,
}

impl OnStart {
    pub(crate) const fn new(thread: Thread) -> Self {
        Self { thread }
    }

    /// Returns the coordinate of the worker that is starting.
    #[must_use]
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

impl super::Hook for OnStart {}

impl super::private::Sealed for OnStart {
    fn into_registration<D>(hook: HookFn<Self, D>) -> HookRegistration
    where
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        HookRegistration::start(hook.erased())
    }
}
