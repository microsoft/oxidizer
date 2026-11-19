// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use crate::{Cycle, DriverError};

/// A worker-local I/O driver.
///
/// The runtime creates one driver for each async worker and calls its methods on that worker.
/// Drivers do not need to implement [`Send`] or [`Sync`]. A driver owns the local state that
/// submits I/O and processes completed operations.
///
/// Dropping a driver must remain memory-safe even when its context still exists.
pub trait Driver: 'static {
    /// Returns a waker that makes a waiting [`execute_cycle`](Self::execute_cycle) return promptly.
    ///
    /// A wake before or during a wait applies to that wait. If no wait is active, the wake remains
    /// latched for the next wait. The elapsed time before that wait does not change this rule. The
    /// wait must stop promptly. The driver can also process ready I/O completions.
    ///
    /// Any thread can call the returned waker. The waker remains safe to call after the driver is
    /// dropped. It must return promptly.
    ///
    /// The returned waker must differ from the owner waker supplied through
    /// [`DriverOptions::owner_waker`](crate::DriverOptions::owner_waker):
    /// [`Waker::will_wake`] must return `false` for that pair. The returned waker must not
    /// synchronously wake the owner waker.
    #[must_use]
    fn waker(&self) -> Waker;

    /// Processes submissions and completions and can wait for I/O.
    ///
    /// The runtime invokes secondary drivers first and always gives them
    /// `Cycle::max_wait() == Duration::ZERO`. A secondary either processes ready completions or
    /// returns immediately when another thread processes its notifications.
    ///
    /// The runtime invokes the primary driver last. The primary can wait for I/O for up to
    /// [`Cycle::max_wait`].
    ///
    /// The driver processes completed operations and returns. It must not loop indefinitely. If
    /// more completed operations are ready, it must call the owner waker from
    /// [`DriverOptions`](crate::DriverOptions) before it returns. A secondary must also arrange a
    /// wake when a later notification requires another cycle. A primary does not need this wake.
    /// Its next `execute_cycle` call performs the worker's I/O wait.
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
    /// async worker. The driver can cancel or disrupt in-flight operations instead of waiting for
    /// them. Remaining cleanup must make progress by itself or on the driver's own threads. It
    /// must not require another driver on that async worker. The driver uses a finite time limit
    /// and returns an error instead of waiting indefinitely.
    ///
    /// The runtime attempts to shut down the remaining drivers after an error. It can shut down
    /// secondaries before the primary.
    ///
    /// Shutdown is best effort. This method reports failures from the implementation. It does not
    /// convert panics into [`DriverError`]. Drivers, custom wakers, diagnostics, and cleanup hooks
    /// must not panic. The runtime does not guarantee recovery from a panic, poisoned state, or
    /// failed invariant.
    ///
    /// # Errors
    ///
    /// Returns an error if operations do not finish and the driver cannot complete shutdown.
    fn shutdown(self) -> Result<(), DriverError>
    where
        Self: Sized;
}
