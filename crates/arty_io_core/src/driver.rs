// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use crate::{Cycle, DriverError};

/// A worker-local I/O driver.
///
/// The runtime calls every method on the owning worker. Drivers need not be [`Send`] or
/// [`Sync`]. Drivers own native observers, queue sharing, routing, and cancellation.
///
/// Dropping a driver must always be memory-safe. State reachable through contexts, callbacks,
/// observers, or wakers must remain valid independently of the driver.
pub trait Driver: 'static {
    /// Returns a waker that interrupts a pending completion wait.
    ///
    /// Wake-ups must be latched: a wake raised before a wait makes the next blocking wait return
    /// promptly, including when the wake originates on the driver's owning worker. Waking ends
    /// only the wait; pending completions still require processing.
    ///
    /// The returned waker may be called from any thread and remains safe to invoke after the
    /// driver is dropped. It must return promptly without joining work or waiting for a lock held
    /// by the completion path.
    #[must_use]
    fn waker(&self) -> Waker;

    /// Processes submissions and completions, optionally waiting for I/O.
    ///
    /// The runtime invokes secondaries before the primary with the same [`Cycle::max_wait`].
    /// Only the primary may wait on the worker, for up to that duration; a zero wait bound means
    /// no waiting. Secondaries may arm background waits within the same bound but must not wait
    /// for them to finish.
    ///
    /// Before publishing a context, the runtime runs a zero-wait cycle. In this initial call,
    /// the driver must establish native notification and recheck work queued during construction.
    ///
    /// The driver must process a bounded batch. If serviceable work remains, it must wake the
    /// owner through the waker supplied in [`DriverOptions`](crate::DriverOptions) before
    /// returning. In-flight operations alone do not indicate serviceable work.
    ///
    /// # Errors
    ///
    /// Returns an error if driver infrastructure fails, not if an individual I/O operation
    /// fails. The runtime rolls back an unpublished driver/context pair on initialization
    /// failure; during normal operation it reports the error and shuts down the worker's drivers.
    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError>;

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
    /// The runtime attempts remaining drivers after an error. It may shut down secondaries before
    /// the primary. The result allows the runtime to report incomplete cleanup consistently.
    ///
    /// This consuming method is intentionally unavailable through `dyn Driver`. A runtime that
    /// erases driver types can use a private owning adapter for storage and shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if graceful cleanup cannot be completed.
    fn shutdown(self) -> Result<(), DriverError>
    where
        Self: Sized;
}
