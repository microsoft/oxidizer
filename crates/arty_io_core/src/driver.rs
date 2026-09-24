// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::{Duration, Instant};

use crate::{DriverHandle, ShutdownError};

/// A thread-local adapter between a runtime worker and an I/O subsystem.
///
/// A runtime creates a driver on its owning worker and invokes every method on that worker.
/// Drivers are not required to implement [`Send`] or [`Sync`].
///
/// State reachable from a context, waker, background thread, or operating-system callback must
/// remain valid independently of the driver. State used only by the owning worker may remain
/// directly in the driver and be accessed through
/// [`process_completions`](Self::process_completions).
///
/// A driver must be safe to drop before, during, or after shutdown. Its associated contexts may
/// outlive it and must reject new operations after admission is closed.
pub trait Driver: 'static {
    /// Returns the handle exposed to drivers registered on the same worker.
    ///
    /// The handle may expose the driver itself or a smaller driver-owned value. The runtime
    /// borrows it only while creating or notifying another driver.
    #[must_use]
    fn handle(&self) -> DriverHandle<'_>;

    /// Notifies this driver that `peer` was registered on the same worker.
    ///
    /// The runtime calls this method after storing the new driver and before completing its
    /// registration. The callback runs on the owning worker and cannot retain `peer`, but it may
    /// clone independently owned state exposed by the handle.
    ///
    /// # Panics
    ///
    /// Implementations must panic if the peer cannot be integrated. The runtime cannot continue
    /// with a partially connected registration.
    fn on_peer_registered(&mut self, _peer: DriverHandle<'_>) {}

    /// Processes completion events, waiting up to `max_wait` for more work.
    ///
    /// [`Duration::ZERO`] performs a non-blocking poll and does not consume a pending wake-up.
    /// [`Duration::MAX`] permits an unbounded wait. Implementations with coarser timing round a
    /// finite duration up without treating it as unbounded.
    ///
    /// The runtime captures `cycle_start` once and passes it unchanged to every driver visited in
    /// the same completion cycle.
    ///
    /// This method must not hold a resource across the wait if another thread needs that resource
    /// to submit work or make a completion available.
    fn process_completions(&mut self, max_wait: Duration, cycle_start: Instant);

    /// Returns a waker for interrupting completion waits.
    ///
    /// Wake-ups are latched. Waking before a blocking wait makes the next blocking call to
    /// [`process_completions`](Self::process_completions) behave like a non-blocking poll. A
    /// wake-up ends only the wait; pending completions are still processed.
    ///
    /// Wake-ups from the owning worker must be observed. Redundant wake-ups may be coalesced, but
    /// no wake-up may be lost. The returned waker remains safe to invoke after the driver is
    /// dropped.
    #[must_use]
    fn waker(&self) -> Waker;

    /// Gracefully shuts down the driver.
    ///
    /// This method closes admission to new operations and waits for active operations and
    /// operating-system callbacks to drain. Context handles do not themselves delay shutdown.
    ///
    /// The implementation must continue making progress on its own completions, must not wait
    /// indefinitely, and must not depend on work that can run only after this method returns.
    ///
    /// [`Drop::drop`] runs after this method returns. Shared cleanup must therefore be idempotent
    /// or otherwise guarded. Regardless of the result, the driver must remain safe to drop and its
    /// contexts must reject new operations.
    ///
    /// # Errors
    ///
    /// Returns an error if graceful cleanup cannot be completed.
    fn shutdown(self) -> Result<(), ShutdownError>;
}
