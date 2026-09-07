// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

use thread_aware_core::Thread;

use crate::SystemTaskSpawner;

/// Placement and runtime facilities supplied when one driver instance is created.
///
/// The fields are private so that future versions can add facilities without preventing existing
/// driver implementations from compiling.
#[derive(Clone)]
pub struct DriverInit {
    thread: Thread,
    system_tasks: Arc<dyn SystemTaskSpawner>,
}

impl DriverInit {
    /// Creates initialization data for one driver instance.
    ///
    /// This constructor is intended for runtime implementations and driver tests.
    #[must_use]
    pub fn new(thread: Thread, system_tasks: Arc<dyn SystemTaskSpawner>) -> Self {
        Self { thread, system_tasks }
    }

    /// Returns the async worker this driver instance serves.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the runtime-owned facility for blocking I/O system work.
    #[must_use]
    pub fn system_tasks(&self) -> &dyn SystemTaskSpawner {
        self.system_tasks.as_ref()
    }
}

impl fmt::Debug for DriverInit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverInit").field("thread", &self.thread).finish_non_exhaustive()
    }
}
