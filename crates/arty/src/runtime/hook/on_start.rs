// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

use super::registry::TypedHook;

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
    pub(crate) fn new(thread: Thread) -> Self {
        Self { thread }
    }

    /// Returns the coordinate of the worker that is starting.
    #[must_use]
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

#[derive(Clone, derive_more::Debug)]
pub(crate) struct StartHook(TypedHook<OnStart>);

impl StartHook {
    pub(crate) fn new<D>(data: D, callback: fn(&mut OnStart, &mut D)) -> Self
    where
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        Self(TypedHook::new(data, callback))
    }
    pub(crate) fn execute(&mut self, event: &mut OnStart) {
        self.0.execute(event);
    }
}

impl thread_aware::ThreadAware for StartHook {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.0.relocate(source, destination);
    }
}
