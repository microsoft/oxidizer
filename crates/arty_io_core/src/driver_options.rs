// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;

use thread_aware_core::Thread;

use crate::DriverRole;

#[derive(Debug)]
/// Inputs for driver creation on one async worker.
pub struct DriverOptions {
    thread: Thread,
    owner_waker: Waker,
    role: DriverRole,
}

impl DriverOptions {
    /// Creates the inputs for a driver on the async worker that `thread` identifies.
    #[must_use]
    pub fn new(thread: Thread, owner_waker: Waker, role: DriverRole) -> Self {
        Self { thread, owner_waker, role }
    }

    /// Returns the async worker that will own the driver.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the waker for the owning async worker.
    ///
    /// A driver must not return this waker from [`Driver::waker`](crate::Driver::waker)
    /// or synchronously wake it from that waker.
    #[must_use]
    pub const fn owner_waker(&self) -> &Waker {
        &self.owner_waker
    }

    /// Returns the role assigned to this driver.
    #[must_use]
    pub const fn role(&self) -> DriverRole {
        self.role
    }
}
