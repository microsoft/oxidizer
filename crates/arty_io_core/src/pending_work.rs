// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::task::Waker;

/// A completion handle for work registered with [`Cycle::start_work`](crate::Cycle::start_work).
///
/// Keep the handle until the work ends, publishing any results before
/// [completing](Self::complete) or dropping it. Both actions send the same notification,
/// interrupting other waits and releasing one participant in the runtime's completion barrier.
///
/// The handle is not cloneable and may move to background work. It adds no allocation or
/// synchronization of its own; those belong to the runtime.
#[must_use = "keep the value alive until its coordinated work has ended"]
pub struct PendingWork {
    on_complete: Waker,
}

impl PendingWork {
    /// Creates a completion handle for work already enrolled in the runtime's current cycle.
    ///
    /// [Completing](Self::complete) or dropping this handle wakes `on_complete` exactly once.
    /// Cloning or dropping the waker itself has no completion meaning.
    ///
    /// The notification must first retire this work's interruption registration, then
    /// interrupt other waits, and finally release only this work's barrier participation.
    /// It must not affect a later cycle.
    ///
    /// The runtime must ensure that the notification can run on any thread, returns promptly
    /// without panicking, and remains memory-safe independently of the driver and runtime worker.
    pub const fn new(on_complete: Waker) -> Self {
        Self { on_complete }
    }

    /// Notifies the runtime that this work has ended.
    ///
    /// Equivalent to dropping the handle; publish any results first.
    #[inline]
    pub fn complete(self) {
        drop(self);
    }
}

impl Drop for PendingWork {
    #[inline]
    fn drop(&mut self) {
        self.on_complete.wake_by_ref();
    }
}

impl fmt::Debug for PendingWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingWork").finish_non_exhaustive()
    }
}
