// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::DriverRole;

/// One worker-local driver, its consumer context, and the role selected by the driver.
///
/// The driver chooses its role from the permission supplied by
/// [`DriverOptions::allowed_roles`](crate::DriverOptions::allowed_roles). A driver may choose
/// [`DriverRole::Secondary`] when the runtime permits [`DriverRole::Primary`], but the runtime
/// rejects a primary driver when primary execution was not permitted.
#[derive(Debug)]
#[non_exhaustive]
#[must_use = "the runtime must initialize and register the driver"]
pub struct DriverInstance<D, C> {
    /// The worker-local driver.
    pub driver: D,
    /// The consumer-facing context associated with the driver.
    pub context: C,
    /// The role selected by the driver.
    pub role: DriverRole,
}

impl<D, C> DriverInstance<D, C> {
    /// Creates a driver instance with the selected `role`.
    pub const fn new(driver: D, context: C, role: DriverRole) -> Self {
        Self { driver, context, role }
    }
}
