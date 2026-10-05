// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{DriverError, DriverInstance, DriverOptions, IoContext, PrimaryDriver, SecondaryDriver};

/// A factory for a [`crate::Driver`] and [`IoContext`] pair on each runtime worker.
///
/// The runtime clones and relocates the provider to each worker, then consumes the clone to
/// create its pair. Shared driver state remains private to the provider.
pub trait DriverProvider: Clone + ThreadAware + Sized + 'static {
    /// The context type associated with this provider.
    type Context: IoContext<Provider = Self>;

    /// The primary driver type created by this provider.
    type Primary: PrimaryDriver;

    /// The secondary driver type created by this provider.
    type Secondary: SecondaryDriver;

    /// Creates a driver and context for the worker described by `options`.
    ///
    /// The runtime calls this method on the owning worker. Implementations must return promptly
    /// without waiting for another runtime worker. The context may outlive the driver and must
    /// reject operations after admission closes.
    ///
    /// Returns a [`DriverInstance::Secondary`](crate::DriverInstance::Secondary), or a
    /// [`DriverInstance::Primary`](crate::DriverInstance::Primary) when
    /// `options.allowed_roles()` permits it. The provider must not publish the context. The
    /// runtime first completes a zero-wait [`PrimaryDriver::execute_cycle`](crate::PrimaryDriver::execute_cycle)
    /// to establish notification and finish primary initialization.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails. Partial state must be safe to drop, with no context
    /// published; the runtime rolls back the pair.
    #[expect(clippy::type_complexity, reason = "the result keeps the role-specific driver types strongly typed")]
    fn create(self, options: DriverOptions) -> Result<DriverInstance<Self::Primary, Self::Secondary, Self::Context>, DriverError>;
}
