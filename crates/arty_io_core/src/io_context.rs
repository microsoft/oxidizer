// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverError, DriverOptions};

/// An application I/O handle that creates worker-local drivers.
///
/// The runtime creates one registration for each concrete context type. The
/// first retrieval creates and registers the context. Each worker creates its
/// driver when it first retrieves that context.
///
/// # Initialization
///
/// When an async worker first retrieves a context, the runtime clones the registered context.
/// It calls [`ThreadAware::relocate`] with `None` as the source and the worker as the destination.
/// The runtime then calls [`driver`](Self::driver). [`DriverOptions::thread`] identifies that same
/// worker.
///
/// Application operations require successful completion of this sequence. Implementations can
/// reject these operations on contexts that did not complete it. Clones of an initialized context
/// remain usable without another `driver` call.
///
/// This sequence applies only to the initial context and driver setup. After initialization,
/// contexts and their clones follow the usual [`ThreadAware`] contract. They remain usable after
/// movement without another `relocate` call. Later relocation affects performance only.
pub trait IoContext: Clone + ThreadAware + 'static {
    /// Configuration used to create the context.
    type ContextOptions;

    /// The driver type created by this context.
    type Driver: Driver;

    /// Creates the context when the runtime first retrieves this concrete context type.
    ///
    /// The runtime calls this method once per registration. It passes the caller's options. The
    /// returned context supplies the clones for [worker-local initialization](Self#initialization).
    #[must_use]
    fn context(options: Self::ContextOptions) -> Self;

    /// Creates a driver for the async worker described by `options`.
    ///
    /// The runtime calls this method on each async worker after it clones and relocates the
    /// context. For each registration, it calls this method at most once per worker. It does not
    /// call this method for each context clone. Implementations can reject repeated calls for the
    /// same registration and worker.
    ///
    /// Implementations must finish all driver setup before they return. They must not wait for
    /// work on another async worker. The context can outlive the driver. It must reject new
    /// operations after [`Driver::shutdown`] closes it.
    ///
    /// The driver must support the role assigned through [`DriverOptions::role`]. Return an error
    /// when it does not.
    ///
    /// # Errors
    ///
    /// Returns an error if driver creation fails. Implementations can also return an error for
    /// repeated calls for the same registration and worker. The runtime does not retry driver
    /// creation. It treats the failure as fatal and starts shutdown on every async worker.
    /// Shutdown closes each context to new operations. Other workers can already have returned
    /// those contexts. The runtime calls [`Driver::shutdown`] on every driver that was created
    /// successfully.
    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError>;
}
