// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use thread_aware_core::Thread;

use crate::{DriverRole, SystemTaskSpawner};

/// Per-worker inputs to [`DriverProvider::create`](crate::DriverProvider::create).
pub struct DriverOptions {
    thread: Thread,
    spawner: SystemTaskSpawner,
    role: DriverRole,
}

impl DriverOptions {
    /// Creates options for a driver on `thread`.
    #[must_use]
    pub fn new(thread: Thread, spawner: SystemTaskSpawner, role: DriverRole) -> Self {
        Self { thread, spawner, role }
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

    /// Returns the most permissive waiting role the runtime grants this driver.
    ///
    /// The driver selects its actual role in the [`DriverInstance`](crate::DriverInstance)
    /// returned by [`DriverProvider::create`](crate::DriverProvider::create). A secondary role
    /// always remains permitted; a primary role is permitted only when this returns
    /// [`DriverRole::Primary`].
    #[must_use]
    pub const fn role(&self) -> DriverRole {
        self.role
    }
}

impl fmt::Debug for DriverOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverOptions")
            .field("thread", &self.thread)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}
