// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::Range;
use std::sync::Arc;

use thread_aware::Thread;

/// An owned iterator over a snapshot of a runtime's worker identities.
///
/// Obtain a fresh snapshot from
/// [`RuntimeControl::workers`](crate::runtime::RuntimeControl::workers).
/// The snapshot records the worker identities at the time of that call.
/// Later runtime changes do not update it. It does not track whether workers
/// are still running.
///
/// Each item is an owned [`Thread`] value. Pass a reference to it to
/// [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at).
/// Neither the snapshot nor its items keep the runtime running or own
/// operating-system resources.
///
/// [`len`](ExactSizeIterator::len) returns the number of remaining items.
/// Before iteration starts, it returns the total worker count in the snapshot.
/// Each yielded item reduces that count by one.
#[derive(Debug)]
pub struct Workers {
    inner: Arc<[Thread]>,
    remaining: Range<usize>,
}

impl Workers {
    pub(crate) fn new(inner: Arc<[Thread]>) -> Self {
        let remaining = 0..inner.len();
        Self { inner, remaining }
    }
}

impl Iterator for Workers {
    type Item = Thread;

    fn next(&mut self) -> Option<Self::Item> {
        self.remaining.next().map(|index| self.inner[index].clone())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.remaining.size_hint()
    }
}

impl ExactSizeIterator for Workers {}
