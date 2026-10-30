// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::slice;

use thread_aware::Thread;

/// A runtime's worker coordinates.
///
/// Obtain this view from
/// [`RuntimeControl::workers`](crate::runtime::RuntimeControl::workers).
/// The runtime owns the view. Each item is a reference into it. Pass an item
/// directly to [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at).
#[derive(Debug)]
pub struct Workers {
    inner: Box<[Thread]>,
}

impl Workers {
    pub(crate) fn new(inner: Box<[Thread]>) -> Self {
        Self { inner }
    }
}

#[expect(
    clippy::into_iter_without_iter,
    reason = "Workers intentionally exposes iteration without public inherent methods"
)]
impl<'a> IntoIterator for &'a Workers {
    type Item = &'a Thread;
    type IntoIter = slice::Iter<'a, Thread>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}
