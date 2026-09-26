// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::fmt;

/// A borrowed, type-erased handle to a driver on the current worker.
///
/// The underlying value can be inspected or downcast through [`Any`]. The handle cannot outlive
/// the registration operation that receives it.
#[derive(Clone, Copy)]
pub struct DriverHandle<'a> {
    driver: &'a dyn Any,
}

impl<'a> DriverHandle<'a> {
    /// Creates a handle to `driver`.
    #[must_use]
    pub const fn new(driver: &'a dyn Any) -> Self {
        Self { driver }
    }

    /// Returns the underlying type-erased value.
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
