// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use crate::{Cycle, DriverError};

/// A worker-local I/O driver.
///
/// The runtime creates one driver for each async worker and calls its methods on that worker.
/// Drivers need not be [`Send`] or [`Sync`]. A driver owns the worker-local state needed to
/// submit I/O and process completed operations.
///
/// Dropping a driver must remain memory-safe even when its context still exists.
pub trait Driver: 'static {
    /// Returns a waker that makes a waiting [`execute_cycle`](Self::execute_cycle) return promptly.
    ///
    /// Any wake that occurs before or during a wait applies to that wait. If no wait is active, it
    /// remains latched for the next wait, regardless of how much time passes before that wait
    /// starts. Waking requires the wait to stop promptly but does not constrain whether the driver
    /// also processes ready I/O completions.
    ///
    /// The returned waker may be called from any thread and remains safe to invoke after the
    /// driver is dropped. It must return promptly.
    #[must_use]
    fn waker(&self) -> Waker;

    /// Processes submissions and completions, optionally waiting for I/O.
    ///
    /// The runtime invokes secondary drivers first and always gives them
    /// `Cycle::max_wait() == Duration::ZERO`. A secondary processes completions that are ready
    /// now, or returns immediately when another thread processes its notifications.
    ///
    /// The runtime invokes the primary driver last. The primary may wait for I/O for up to
    /// [`Cycle::max_wait`].
    ///
    /// The driver processes completed operations and returns rather than looping indefinitely.
    /// If more completed operations are already ready when it returns, it must call the owner
    /// waker from [`DriverOptions`](crate::DriverOptions). A secondary that needs another cycle
    /// when a later notification arrives must also arrange for that waker to be called. A primary
    /// does not need to wake the worker for later completions because its next `execute_cycle`
    /// call performs the worker's I/O wait.
    ///
    /// # Errors
    ///
    /// Returns an error if the driver cannot continue. The runtime treats this as fatal and starts
    /// runtime shutdown. An individual application I/O failure belongs to that operation's result,
    /// not this method.
    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError>;

    /// Closes the context to new operations and releases the driver's resources.
    ///
    /// The runtime calls this method synchronously after it stops calling `execute_cycle` on the
    /// async worker. Operations still in flight are no longer assumed to be valuable; the driver
    /// may cancel or disrupt them instead of waiting for them to finish. Any cleanup that remains
    /// must make progress by itself or on the driver's own threads and must not require another
    /// driver on that async worker to run. The driver chooses a finite time limit and returns an
    /// error instead of waiting indefinitely.
    ///
    /// The runtime attempts remaining drivers after an error. It may shut down secondaries before
    /// the primary.
    ///
    /// # Errors
    ///
    /// Returns an error if operations do not finish and the driver cannot complete shutdown.
    fn shutdown(self) -> Result<(), DriverError>
    where
        Self: Sized;
}
