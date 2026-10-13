// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverError, DriverInstance, DriverOptions};

/// A consumer-facing I/O handle that creates worker-local drivers.
///
/// The runtime registers each concrete context type once. On its first request, it creates the
/// context, clones and relocates it to every active worker, creates a driver on each worker, and
/// completes each driver's zero-wait initialization cycle before publishing the context. Later
/// requests reuse the registration.
///
pub trait IoContext: Clone + ThreadAware + 'static {
    /// Configuration used to create the context.
    type ContextOptions;

    /// The driver type created by this context.
    type Driver: Driver;

    /// Creates the context for a registration.
    ///
    /// The runtime calls this method at most once per registered context type, passing
    /// `options`. Repeated calls should return equivalent contexts.
    #[must_use]
    fn context(options: Self::ContextOptions) -> Self;

    /// Creates a driver for the worker described by `options`.
    ///
    /// The runtime calls this method on the owning worker after cloning and relocating the
    /// context. Implementations must return promptly without waiting for another runtime worker.
    /// The context may outlive the driver and must reject operations after admission closes.
    ///
    /// The context must not be published yet. The runtime first completes a zero-wait
    /// [`Driver::execute_cycle`] to establish notification and finish initialization.
    ///
    /// A successful instance must select a role listed by [`DriverOptions::allowed_roles`].
    /// Return an error when the driver supports none of the allowed roles.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails. The implementation must clean up partial
    /// construction before returning an error. If the successfully returned driver's initial
    /// cycle fails, the runtime drops that unpublished driver without publishing the context.
    fn driver(&mut self, options: DriverOptions) -> Result<DriverInstance<Self::Driver>, DriverError>;
}
