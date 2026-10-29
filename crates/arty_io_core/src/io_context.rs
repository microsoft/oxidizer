// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverError, DriverOptions};

/// A consumer-facing I/O handle that creates worker-local drivers.
///
/// The runtime creates one registration for each concrete context type. The
/// first retrieval creates and registers the context. Each worker then creates
/// its driver lazily before returning that context from the worker-local host.
///
/// # Initialization
///
/// When an async worker first retrieves a context, the runtime clones the registered context.
/// It calls [`ThreadAware::relocate`] with `None` as the source and the worker as
/// the destination, then calls [`driver`](Self::driver). [`DriverOptions::thread`]
/// identifies that same worker.
///
/// Application-facing operations require successful completion of this sequence.
/// Implementations can immediately reject all such operations on contexts that did
/// not complete it. Clones of an initialized context remain usable without another
/// `driver` call.
///
/// This sequence applies only to initial I/O context and driver setup. After
/// initialization, contexts and their clones follow the usual [`ThreadAware`]
/// contract. They remain usable after movement without another `relocate` call.
/// Later relocation affects performance only.
pub trait IoContext: Clone + ThreadAware + 'static {
    /// Configuration used to create the context.
    type ContextOptions;

    /// The driver type created by this context.
    type Driver: Driver;

    /// Creates the context for the first retrieval of this concrete context type.
    ///
    /// The runtime calls this method once per registration and passes the options supplied by the caller.
    /// The returned context supplies the clones used for [worker-local initialization](Self#initialization).
    #[must_use]
    fn context(options: Self::ContextOptions) -> Self;

    /// Creates a driver for the async worker described by `options`.
    ///
    /// The runtime calls this method on each async worker after cloning and relocating the context.
    /// For each registration, the runtime calls this method at most once per async worker,
    /// not once per context clone. Implementations can reject repeated calls for the same
    /// registration and worker.
    ///
    /// Implementations must finish all driver setup before returning and must not wait for work to
    /// execute on another async worker. The context can outlive the driver and must reject new
    /// operations after [`Driver::shutdown`] closes it.
    ///
    /// The driver must support the role assigned through [`DriverOptions::role`]. Return an error
    /// when it does not.
    ///
    /// # Errors
    ///
    /// Returns an error if driver creation fails. Implementations can also return an
    /// error for repeated calls for the same registration and worker. The runtime does not retry
    /// driver creation. It treats the failure as fatal and starts runtime shutdown on
    /// every async worker. Contexts that other workers already returned must reject new
    /// operations after shutdown closes them. The runtime calls
    /// [`Driver::shutdown`] on every driver that was created successfully.
    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError>;
}
