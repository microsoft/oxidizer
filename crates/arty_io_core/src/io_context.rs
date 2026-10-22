// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverError, DriverOptions};

/// A consumer-facing I/O handle that creates worker-local drivers.
///
/// The runtime creates one registration for each concrete context type. The
/// first retrieval creates and registers the context. Each worker then creates
/// its driver lazily before returning that context from the worker-local host.
pub trait IoContext: Clone + ThreadAware + 'static {
    /// Configuration used to create the context.
    type ContextOptions;

    /// The driver type created by this context.
    type Driver: Driver;

    /// Creates the context for the first retrieval of this concrete context type.
    ///
    /// The runtime calls this method once and passes the options supplied by the caller.
    #[must_use]
    fn context(options: Self::ContextOptions) -> Self;

    /// Creates a driver for the async worker described by `options`.
    ///
    /// The runtime calls this method on each async worker after cloning and relocating the context.
    /// Implementations must finish all driver setup before returning and must not wait for work to
    /// execute on another async worker. The context may outlive the driver and must reject new
    /// operations after [`Driver::shutdown`] closes it.
    ///
    /// The driver must support the role assigned through [`DriverOptions::role`]. Return an error
    /// when it does not.
    ///
    /// # Errors
    ///
    /// Returns an error if driver creation fails. The implementation must clean
    /// up partial construction before returning. The runtime treats the failure
    /// as fatal and starts runtime shutdown on every async worker. Contexts may
    /// already have been returned by other workers; they must reject new
    /// operations after shutdown closes them. The runtime calls
    /// [`Driver::shutdown`] on every driver that was created successfully.
    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError>;
}
