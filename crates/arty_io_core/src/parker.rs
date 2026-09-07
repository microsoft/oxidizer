// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

/// A completion-aware waiting point that an I/O driver lends to an async worker.
///
/// Only one driver can provide a worker's waiting point. Drivers that do not provide one arrange
/// progress through runtime blocking workers or threads of their own.
pub trait Parker {
    /// Waits for at most `max_wait`, processes available completions, and returns.
    ///
    /// [`Duration::ZERO`] requests a non-blocking poll. [`Duration::MAX`] requests an unbounded
    /// wait. A mechanism with coarser timing rounds finite waits up without converting one into an
    /// unbounded wait. Returning early is always allowed.
    ///
    /// Operations may be submitted from other threads while this method waits. The implementation
    /// must not hold anything across the wait that a submitter needs.
    fn park(&mut self, max_wait: Duration);

    /// Returns a handle that causes the current or next [`park`](Self::park) call to return.
    ///
    /// Wakeups are latched: a wake raised before a wait makes the next wait return immediately.
    /// Same-thread wakeups are honored. Redundant wakeups may be coalesced, but a wakeup is never
    /// dropped. The returned waker remains safe to invoke after the parker is dropped.
    fn waker(&self) -> Waker;
}
