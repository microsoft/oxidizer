// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{PrimaryDriver, SecondaryDriver};

/// One worker-local driver, its consumer context, and the role selected by the driver.
///
/// The driver chooses its role from the permission supplied by
/// [`DriverOptions::allowed_roles`](crate::DriverOptions::allowed_roles). A driver may choose
/// [`crate::DriverRole::Secondary`] when the runtime permits [`crate::DriverRole::Primary`].
#[derive(Debug)]
#[non_exhaustive]
#[must_use = "the runtime must initialize and register the driver"]
pub enum DriverInstance<P, S, C>
where
    P: PrimaryDriver,
    S: SecondaryDriver,
{
    /// A driver polled by the runtime worker.
    Primary {
        /// The worker-local primary driver.
        driver: P,
        /// The consumer-facing context associated with the driver.
        context: C,
    },
    /// A driver serviced independently of the runtime polling loop.
    Secondary {
        /// The worker-local secondary driver.
        driver: S,
        /// The consumer-facing context associated with the driver.
        context: C,
    },
}

impl<P, S, C> DriverInstance<P, S, C>
where
    P: PrimaryDriver,
    S: SecondaryDriver,
{
    /// Creates a primary driver instance.
    pub const fn primary(driver: P, context: C) -> Self {
        Self::Primary { driver, context }
    }

    /// Creates a secondary driver instance.
    pub const fn secondary(driver: S, context: C) -> Self {
        Self::Secondary { driver, context }
    }
}
