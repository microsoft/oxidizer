// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use thread_aware_core::Thread;

use crate::DriverRole;

#[derive(Debug)]
/// Per-worker inputs to [`IoContext::driver`](crate::IoContext::driver).
pub struct DriverOptions {
    thread: Thread,
    owner_waker: Waker,
    allowed_roles: Vec<DriverRole>,
}

impl DriverOptions {
    /// Creates options for a driver on `thread`.
    #[must_use]
    pub fn new(thread: Thread, owner_waker: Waker, allowed_roles: Vec<DriverRole>) -> Self {
        Self {
            thread,
            owner_waker,
            allowed_roles,
        }
    }

    /// Returns the worker that will own the driver.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the waker for requesting another cycle from the runtime.
    #[must_use]
    pub const fn owner_waker(&self) -> &Waker {
        &self.owner_waker
    }

    /// Returns the waiting roles the runtime can accept from this driver.
    #[must_use]
    pub fn allowed_roles(&self) -> &[DriverRole] {
        &self.allowed_roles
    }
}
