// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

/// Reports that an async worker is starting.
///
/// Arty relocates the callback data to the worker before it creates this event.
/// The callback runs after the worker installs its scheduler and worker-local
/// services. It runs before the worker reports readiness, before
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns,
/// and before the runtime accepts tasks.
///
/// A late registration through
/// [`RuntimeControl::on`](crate::runtime::RuntimeControl::on) replaces the
/// stored callback. It does not create an event because all workers have
/// already started.
#[derive(derive_more::Debug)]
pub struct OnStart {
    pub(super) thread: Thread,
}

impl OnStart {
    /// Returns the coordinate of the worker that is starting.
    #[must_use]
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}
