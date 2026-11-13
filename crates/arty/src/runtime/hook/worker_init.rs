// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

/// An event that runs once on each async worker after the worker initializes.
///
/// Each registered callback runs exactly once on every worker. This includes
/// workers that start after callback registration.
#[derive(Debug)]
pub struct WorkerInit {
    thread: Thread,
}

impl WorkerInit {
    pub(crate) const fn new(thread: Thread) -> Self {
        Self { thread }
    }

    /// Returns the coordinate of the worker thread being initialized.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }
}

impl super::Hook for WorkerInit {}

impl super::private::Sealed for WorkerInit {
    type Erased<D>
        = super::hook_fn::ErasedHookFn<Self>
    where
        D: thread_aware::ThreadAware + Clone + 'static;
}
