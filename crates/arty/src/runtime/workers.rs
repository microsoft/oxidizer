// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::Thread;

/// A list of a runtime's workers and their details.
///
/// Obtain this list from
/// [`RuntimeControl::workers`](crate::runtime::RuntimeControl::workers).
/// The runtime owns the list. Each item identifies one worker. Pass a reference to an item
/// directly to [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at).
#[derive(Debug)]
pub struct Workers {
    inner: Box<[Thread]>,
}

impl Workers {
    pub(crate) fn new(inner: Box<[Thread]>) -> Self {
        Self { inner }
    }

    /// Returns the number of workers in this runtime.
    #[must_use]
    #[expect(clippy::len_without_is_empty, reason = "runtime worker lists are non-empty by construction")]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns an iterator over owned worker thread handles.
    ///
    /// Each item is a clone of a handle in the list. Iteration does not remove
    /// workers or transfer ownership of the runtime's threads.
    pub fn iter(&self) -> impl Iterator<Item = Thread> {
        self.inner.iter().cloned()
    }
}
