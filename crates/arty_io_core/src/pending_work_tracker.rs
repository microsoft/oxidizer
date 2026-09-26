// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use crate::PendingWork;

/// Runtime-managed registration for pending driver work.
///
/// A [`Cycle`](crate::Cycle) borrows the tracker mutably. Registration runs on the owning
/// worker, so implementations need not be [`Send`] or [`Sync`].
///
/// The runtime begins coordination once per cycle, before checking work. After the primary
/// returns, it interrupts remaining waits and waits for every [`PendingWork`] to complete or
/// drop before advancing. Interruption state, counters, and parking are runtime-owned.
pub trait PendingWorkTracker {
    /// Registers one unit of pending work and its interruption waker.
    ///
    /// Implementations must enroll the work in the current cycle's completion barrier and
    /// register `interrupt` before returning. If the cycle is already interrupted, they must
    /// invoke the waker before returning. Registration must not wait for the work to finish.
    ///
    /// The returned handle must implement the notification contract in [`PendingWork::new`].
    /// See [`Cycle::start_work`](crate::Cycle::start_work) for native wait requirements.
    fn start_work(&mut self, interrupt: Waker) -> PendingWork;
}
