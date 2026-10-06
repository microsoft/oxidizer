// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use thread_aware_core::Thread;

use crate::{DriverRole, SystemTaskSpawner};

/// Per-worker inputs to [`DriverProvider::create`](crate::DriverProvider::create).
pub struct DriverOptions {
    thread: Thread,
    spawner: SystemTaskSpawner,
    allowed_roles: Vec<DriverRole>,
}

impl DriverOptions {
    /// Creates options for a driver on `thread`.
    #[must_use]
    pub fn new(thread: Thread, spawner: SystemTaskSpawner, allowed_roles: Vec<DriverRole>) -> Self {
        Self {
            thread,
            spawner,
            allowed_roles,
        }
    }

    /// Returns the worker that will own the driver.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the spawner for blocking system work.
    #[must_use]
    pub const fn spawner(&self) -> &SystemTaskSpawner {
        &self.spawner
    }

    /// Returns the waiting roles the runtime permits this driver to select.
    ///
    /// The driver selects its actual role in the [`DriverInstance`](crate::DriverInstance)
    /// returned by [`DriverProvider::create`](crate::DriverProvider::create).
    #[must_use]
    pub fn allowed_roles(&self) -> &[DriverRole] {
        &self.allowed_roles
    }
}

impl fmt::Debug for DriverOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverOptions")
            .field("thread", &self.thread)
            .field("allowed_roles", &self.allowed_roles)
            .finish_non_exhaustive()
    }
}
