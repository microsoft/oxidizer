// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use thread_aware_core::Thread;

use crate::SystemTasks;

/// Placement and runtime facilities supplied when one driver instance is created.
///
/// The fields are private so that future versions can add facilities without preventing existing
/// driver implementations from compiling.
pub struct DriverContext {
    thread: Thread,
    system_tasks: SystemTasks,
}

impl DriverContext {
    /// Creates the context for one driver instance.
    ///
    /// This constructor is intended for runtime implementations and driver tests.
    #[must_use]
    pub fn new(thread: Thread, system_tasks: SystemTasks) -> Self {
        Self { thread, system_tasks }
    }

    /// Returns the async worker this driver instance serves.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the runtime-owned facility for blocking I/O system work.
    #[must_use]
    pub const fn system_tasks(&self) -> &SystemTasks {
        &self.system_tasks
    }
}

impl fmt::Debug for DriverContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverContext")
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}
