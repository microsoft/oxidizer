// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverError, DriverInstance, DriverOptions, IoContext};

/// A factory for a [`Driver`] and [`IoContext`] pair on each runtime worker.
///
/// The runtime clones and relocates the provider to each worker, then consumes the clone to
/// create its pair. Shared driver state remains private to the provider.
pub trait DriverProvider: Clone + ThreadAware + Sized + 'static {
    /// The context type associated with this provider.
    type Context: IoContext<Provider = Self>;

    /// The driver type created by this provider.
    type Driver: Driver;

    /// Creates a driver and context for the worker described by `options`.
    ///
    /// The runtime calls this method on the owning worker. Implementations must return promptly
    /// without waiting for another runtime worker. The context may outlive the driver and must
    /// reject operations after admission closes.
    ///
    /// Returns a [`DriverInstance`] whose role must be
    /// [`DriverRole::Secondary`](crate::DriverRole::Secondary), or
    /// [`DriverRole::Primary`](crate::DriverRole::Primary) when `options.role()` permits it.
    /// The provider must not publish the context. The runtime first completes a zero-wait
    /// [`Driver::execute_cycle`] to establish notification and finish initialization.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails. Partial state must be safe to drop, with no context
    /// published; the runtime rolls back the pair.
    fn create(self, options: DriverOptions) -> Result<DriverInstance<Self::Driver, Self::Context>, DriverError>;
}
