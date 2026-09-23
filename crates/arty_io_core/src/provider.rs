// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverOptions, IoContext};

/// A factory for a driver's per-worker instances.
///
/// A runtime clones and relocates the provider for each worker, then consumes the relocated clone
/// to create that worker's driver. State shared by driver instances remains private to the
/// provider.
pub trait DriverProvider: Clone + ThreadAware + Sized + 'static {
    /// The context type associated with this provider.
    type Context: IoContext<Provider = Self>;

    /// The driver type created by this provider.
    type Driver: Driver<Context = Self::Context>;

    /// Creates a driver for the worker described by `options`.
    ///
    /// The runtime calls this method on the worker that will own the driver. The implementation
    /// must return promptly and must not wait for another runtime worker to make progress.
    ///
    /// [`DriverOptions::drivers`](crate::DriverOptions::drivers) contains borrowed handles to
    /// drivers registered earlier on the same worker. The new driver may clone independently owned
    /// state from those handles.
    ///
    /// # Panics
    ///
    /// Implementations must panic if the driver cannot be created. The runtime cannot continue
    /// with a partially completed registration.
    fn create(self, options: DriverOptions<'_>) -> Self::Driver;
}
