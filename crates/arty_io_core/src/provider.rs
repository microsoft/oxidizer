// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;

use crate::{Driver, DriverInit, ThreadAware};

/// Creates and connects one driver type's per-worker instances.
///
/// A runtime keeps one provider per registered driver type. For each worker it clones and
/// relocates the provider, then consumes that clone to create the worker's driver instance.
/// Shared queues, registries, and driver-owned threads remain private provider state.
pub trait DriverProvider: Clone + ThreadAware + 'static {
    /// The driver type created by this provider.
    type Driver: Driver;

    /// The error returned when a driver instance cannot be created.
    type Error: Error + Send + Sync + 'static;

    /// Creates the driver instance serving one async worker.
    ///
    /// This method runs on the thread that will own the returned driver. It must return promptly
    /// and must not wait for async workers to make progress. Consuming the relocated provider clone
    /// makes the one-creation-per-worker lifecycle explicit.
    fn create(self, init: DriverInit) -> Result<Self::Driver, Self::Error>;
}
