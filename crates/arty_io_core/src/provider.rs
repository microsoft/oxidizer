// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverError, DriverOptions, IoContext};

/// A factory for per-worker driver and context pairs.
///
/// A runtime clones and relocates the provider for each worker, then consumes the relocated clone
/// to create that worker's pair. State shared by driver instances remains private to the provider.
pub trait DriverProvider: Clone + ThreadAware + Sized + 'static {
    /// The context type associated with this provider.
    type Context: IoContext<Provider = Self>;

    /// The driver type created by this provider.
    type Driver: Driver;

    /// Creates a driver and context for the worker described by `options`.
    ///
    /// The runtime calls this method on the worker that will own the driver. The implementation
    /// must return promptly and must not wait for another runtime worker to make progress.
    /// The context may outlive the driver and must reject new operations after admission is
    /// closed.
    ///
    /// [`DriverOptions::drivers`](crate::DriverOptions::drivers) contains borrowed handles to
    /// drivers registered earlier on the same worker. The new driver may clone independently owned
    /// state from those handles.
    ///
    /// Prepare native resources without publishing the context. The runtime then invokes an
    /// initial zero-wait [`Driver::execute_cycle`] to supply the stable interruptor, connect
    /// notification, and recheck early work before publishing the context or notifying peers.
    ///
    /// # Errors
    ///
    /// Returns an error if the driver and context cannot be initialized. On error, partially
    /// initialized native state must be safe to drop and no usable context may have been
    /// published. The runtime rolls back the unpublished pair.
    fn create(self, options: DriverOptions<'_>) -> Result<(Self::Driver, Self::Context), DriverError>;
}
