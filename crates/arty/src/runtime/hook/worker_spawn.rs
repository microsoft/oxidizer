// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

/// An event that runs once on each async worker after the worker initializes.
/// [^worker-spawn-retention]
///
/// Do not request runtime shutdown from this callback, including through
/// [`RuntimeControl::shutdown`](crate::runtime::RuntimeControl::shutdown).
/// Worker initialization callbacks must finish their setup and return normally.
///
/// [^worker-spawn-retention]: Arty retains each registered callback. The callback runs once on every
/// current worker and once on each worker that starts after registration.
#[derive(Debug)]
pub struct WorkerSpawn {
    thread: Thread,
}

impl WorkerSpawn {
    pub(crate) const fn new(thread: Thread) -> Self {
        Self { thread }
    }

    /// Returns the coordinate of the worker thread being initialized.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }
}

impl super::Hook for WorkerSpawn {}

impl super::private::Sealed for WorkerSpawn {}
