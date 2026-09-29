// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{Cycle, DriverError, ShutdownError};

/// A worker-local I/O driver.
///
/// The runtime calls every method on the owning worker. Drivers need not be [`Send`] or
/// [`Sync`]. Drivers own native observers, queue sharing, routing, and cancellation.
///
/// Dropping a driver must always be memory-safe. State reachable through contexts, callbacks,
/// observers, or wakers must remain valid independently of the driver.
pub trait Driver: 'static {
    /// Processes submissions and completions, optionally waiting for I/O.
    ///
    /// The runtime invokes secondaries before the primary, sharing [`Cycle::started_at`] and
    /// [`Cycle::max_wait`]. Only an invocation with [`Cycle::can_block`] set to `true` may block
    /// the worker. Secondaries may arm background waits but must not wait for them to finish.
    ///
    /// Before publishing a context, the runtime runs a zero-wait cycle with `can_block` set to
    /// `false`. In this initial call, the driver must establish native notification and recheck
    /// work queued during construction.
    ///
    /// Register each native wait with [`Cycle::start_work`] before entering or scheduling it.
    /// Keep its [`PendingWork`](crate::PendingWork) alive until the work ends. The runtime
    /// interrupts remaining waits after the primary returns and waits for every handle to
    /// complete or drop before starting the next cycle. An interrupted wait still requires
    /// completion processing.
    ///
    /// The driver must process a bounded batch. If serviceable work remains, it must complete
    /// a pending-work handle before returning. In-flight operations alone do not indicate
    /// serviceable work.
    ///
    /// # Errors
    ///
    /// Returns an error if driver infrastructure fails, not if an individual I/O operation
    /// fails. The runtime rolls back an unpublished driver/context pair on initialization
    /// failure; during normal operation it reports the error and shuts down the worker's drivers.
    fn execute_cycle(&mut self, cycle: &mut Cycle<'_>) -> Result<(), DriverError>;

    /// Closes admission and blocks for a bounded time while draining driver resources.
    ///
    /// Drain active operations, callbacks, and observers, or return an error. Context handles
    /// remain valid as closed handles and do not themselves delay shutdown.
    ///
    /// The driver must make progress locally or on independent threads. Normal cycles have
    /// stopped: shutdown must not depend on cycle coordination or another driver on the same worker.
    ///
    /// Dropping must remain memory-safe after either result. Cancellation alone does not prove
    /// that native code has stopped accessing operation storage.
    ///
    /// The runtime keeps [`SystemTaskSpawner`](crate::SystemTaskSpawner) available until all
    /// shutdown calls return and attempts remaining drivers after an error. It may shut down
    /// secondaries before the primary.
    ///
    /// # Errors
    ///
    /// Returns an error if graceful cleanup cannot be completed.
    fn shutdown(self) -> Result<(), ShutdownError>
    where
        Self: Sized;
}
