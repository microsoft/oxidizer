// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use thread_aware_core::Thread;

use crate::{DriverHandle, DriverRole, SystemTaskSpawner};

/// Per-worker inputs to [`DriverProvider::create`](crate::DriverProvider::create).
///
/// These options may borrow thread-local drivers and are therefore neither [`Send`] nor [`Sync`].
pub struct DriverOptions<'a> {
    thread: Thread,
    spawner: SystemTaskSpawner,
    drivers: Vec<DriverHandle<'a>>,
    role: DriverRole,
}

impl<'a> DriverOptions<'a> {
    /// Creates options for a driver on `thread`.
    #[must_use]
    pub fn new(thread: Thread, spawner: SystemTaskSpawner, drivers: Vec<DriverHandle<'a>>, role: DriverRole) -> Self {
        Self {
            thread,
            spawner,
            drivers,
            role,
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

    /// Returns handles to drivers registered earlier on this worker.
    ///
    /// The handles appear in runtime-defined order and are valid only for the current call to
    /// [`DriverProvider::create`](crate::DriverProvider::create). A driver may downcast a handle
    /// and clone independently owned state from it.
    #[must_use]
    pub fn drivers(&self) -> &[DriverHandle<'a>] {
        &self.drivers
    }

    /// Returns this driver's runtime-assigned waiting role.
    ///
    /// The role is fixed for the driver's lifetime. See [`DriverRole`] for waiting rules.
    #[must_use]
    pub const fn role(&self) -> DriverRole {
        self.role
    }
}

impl fmt::Debug for DriverOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverOptions")
            .field("thread", &self.thread)
            .field("driver_count", &self.drivers.len())
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}
