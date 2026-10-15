// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use thread_aware_core::Thread;

use crate::DriverRole;

#[derive(Debug)]
/// Inputs for creating a driver on one async worker.
pub struct DriverOptions {
    thread: Thread,
    owner_waker: Waker,
    allowed_roles: Vec<DriverRole>,
}

impl DriverOptions {
    /// Creates options for a driver on the async worker represented by `thread`.
    #[must_use]
    pub fn new(thread: Thread, owner_waker: Waker, allowed_roles: Vec<DriverRole>) -> Self {
        Self {
            thread,
            owner_waker,
            allowed_roles,
        }
    }

    /// Returns the async worker that will own the driver.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the waker that wakes the owning async worker.
    #[must_use]
    pub const fn owner_waker(&self) -> &Waker {
        &self.owner_waker
    }

    /// Returns the roles that the runtime can accept for this registration.
    #[must_use]
    pub fn allowed_roles(&self) -> &[DriverRole] {
        &self.allowed_roles
    }
}
