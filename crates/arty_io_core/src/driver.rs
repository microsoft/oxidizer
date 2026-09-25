// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{Cycle, DriverError, DriverHandle, ShutdownError};

/// A worker-local integration point for an independently implemented I/O subsystem.
///
/// The runtime creates the driver and invokes every method on its owning worker. Drivers need
/// not implement `Send` or `Sync`. Native observers, queue sharing, routing, and cancellation
/// belong to the implementation, not the runtime.
///
/// State reachable from contexts, callbacks, observers, or wakers must remain valid independently
/// of the driver. Dropping a driver at any lifecycle point is memory-safe.
pub trait Driver: 'static {
    /// Returns a borrowed handle for discovering compatible drivers on the same worker.
    ///
    /// A peer may inspect the handle and clone independently owned state, but cannot retain the
    /// borrow. Driver-owned coordination can use these handles without a core wait registry.
    #[must_use]
    fn handle(&self) -> DriverHandle<'_>;

    /// Notifies this driver of a newly registered peer before its context is published.
    ///
    /// # Panics
    ///
    /// Panic if the peer cannot be integrated. The runtime cannot continue a partially connected
    /// registration. Native initialization failures belong in provider creation or the initial
    /// cycle instead.
    fn on_peer_registered(&mut self, peer: DriverHandle<'_>);

    /// Processes submissions and completions and optionally waits for native work.
    ///
    /// The runtime invokes every secondary before the single primary, using the same
    /// [`Cycle::started_at`] and [`Cycle::max_wait`] for every call. A primary may apply that
    /// duration directly to its worker wait. A secondary may use the duration only to arm or
    /// replace an off-worker wait; its worker-local call must return without waiting for that
    /// background operation to finish. It starts coordination for that work; the runtime does not
    /// begin the next cycle until every token calls `work_completed` after publishing work or is
    /// dropped after ending without work.
    ///
    /// Registration includes an initial zero-wait cycle before the context is published or peers
    /// are notified. Obtain a stable interruption waker from the cycle, connect native
    /// notification, and recheck work queued during construction. Failure aborts registration.
    ///
    /// Start coordination for each current native wait, attach that wait's waker with
    /// [`CoordinationToken::on_interrupted`](crate::CoordinationToken::on_interrupted) before
    /// checking [`CoordinationToken::is_interrupted`](crate::CoordinationToken::is_interrupted)
    /// or entering the wait, and keep the token alive until the wait finishes. Native
    /// interruption must be latched across that transition. Waiting ends only the wait; pending
    /// completions still need processing.
    ///
    /// Process a bounded batch. If that bound is reached while immediately serviceable work
    /// remains, call [`CoordinationToken::work_completed`](crate::CoordinationToken::work_completed)
    /// before returning. Do not interrupt another cycle merely because operations remain in
    /// flight or because a wait was interrupted.
    ///
    /// # Errors
    ///
    /// Returns an infrastructure failure. During registration the runtime rolls back the
    /// unpublished driver/context pair. During normal operation it reports the error and shuts
    /// down the worker's drivers. Individual failed I/O operations retain their own results.
    fn execute_cycle(&mut self, cycle: Cycle<'_>) -> Result<(), DriverError>;

    /// Gracefully shuts down the driver.
    ///
    /// This method consumes the driver, closes admission, and blocks until active operations,
    /// callbacks, and driver-owned observers drain or cleanup fails. Context handles do not
    /// themselves delay shutdown.
    ///
    /// The driver must bound its shutdown wait and make all required progress itself or on
    /// independently running threads. It must not depend on another driver serialized on the
    /// same runtime worker, regardless of shutdown order. The shared cycle coordinator is no
    /// longer driven after normal cycle processing stops and must not be the sole notification
    /// mechanism for shutdown progress.
    ///
    /// Dropping after success or failure remains memory-safe. Cancellation alone is not proof
    /// that native code has stopped accessing operation storage.
    ///
    /// The runtime invokes shutdown on the owning worker, keeps
    /// [`SystemTaskSpawner`](crate::SystemTaskSpawner) available until every shutdown call
    /// returns, and attempts the remaining drivers after an error. It may shut down secondaries
    /// before the primary to preserve primary-owned infrastructure longest.
    ///
    /// # Errors
    ///
    /// Returns an error if graceful cleanup cannot be completed.
    fn shutdown(self) -> Result<(), ShutdownError>
    where
        Self: Sized;
}
