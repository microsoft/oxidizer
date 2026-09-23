// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::fmt;

/// A registration-time view of a driver on the current thread.
///
/// A runtime supplies these through
/// [`DriverOptions::drivers`](crate::DriverOptions::drivers) and
/// [`Driver::on_peer_registered`](crate::Driver::on_peer_registered). The handle can be
/// inspected or downcast through [`Any`] but cannot outlive the call that received it.
#[derive(Clone, Copy)]
pub struct DriverHandle<'a> {
    driver: &'a dyn Any,
}

impl<'a> DriverHandle<'a> {
    /// Creates a handle for an already registered driver.
    ///
    /// This constructor is intended for driver implementations and tests.
    #[must_use]
    pub const fn new(driver: &'a dyn Any) -> Self {
        Self { driver }
    }

    /// Returns the type-erased handle exposed by the registered driver.
    #[must_use]
    pub const fn handle(&self) -> &'a dyn Any {
        self.driver
    }
}

impl fmt::Debug for DriverHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverHandle")
            .field("type_id", &self.driver.type_id())
            .finish_non_exhaustive()
    }
}
