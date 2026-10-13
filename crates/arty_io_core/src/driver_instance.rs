// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{Driver, DriverRole};

/// A worker-local driver and its fixed waiting role.
#[derive(Debug)]
#[non_exhaustive]
pub struct DriverInstance<D> {
    /// The worker-local driver.
    pub driver: D,
    /// The driver's fixed waiting role.
    pub role: DriverRole,
}

impl<D: Driver> DriverInstance<D> {
    /// Bundles `driver` with its waiting `role`.
    #[must_use]
    pub const fn new(driver: D, role: DriverRole) -> Self {
        Self { driver, role }
    }
}
