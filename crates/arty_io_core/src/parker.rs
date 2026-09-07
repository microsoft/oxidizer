// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

/// A completion-aware waiting point that an I/O driver lends to an async worker.
///
/// Every driver provides one. The runtime decides whether to integrate it into an async worker's
/// idle wait or drive it from another runtime-owned thread. The driver remains free to delegate
/// actual completion work to system tasks or threads of its own.
pub trait Parker {
    /// Waits for at most `max_wait`, processes available completions, and returns.
    ///
    /// [`Duration::ZERO`] requests a non-blocking poll. [`Duration::MAX`] requests an unbounded
    /// wait. A mechanism with coarser timing rounds finite waits up without converting one into an
    /// unbounded wait. Returning early is always allowed.
    ///
    /// Operations may be submitted from other threads while this method waits. The implementation
    /// must not hold anything across the wait that a submitter needs.
    fn park(&self, max_wait: Duration);

    /// Returns a handle that causes the current or next [`park`](Self::park) call to return.
    ///
    /// Wake-ups are latched: a wake raised before a wait makes the next wait return immediately.
    /// Same-thread wake-ups are honored. Redundant wake-ups may be coalesced, but a wake-up is never
    /// dropped. The returned waker remains safe to invoke after the [`Parker`] is dropped.
    fn waker(&self) -> Waker;
}
