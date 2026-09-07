// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::Arc;

use crate::{BlockingTaskSpawner, Thread};

/// Whether the runtime worker's waiting point is available to a driver instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WaitingPoint {
    /// The driver may expose a [`Parker`](crate::Parker) and let the worker wait inside it.
    Available,

    /// The driver must arrange progress without owning the worker's waiting point.
    Unavailable,
}

/// Placement and runtime facilities supplied when one driver instance is created.
///
/// The fields are private so that future versions can add facilities without preventing existing
/// driver implementations from compiling.
#[derive(Clone)]
pub struct DriverInit {
    thread: Thread,
    blocking_tasks: Arc<dyn BlockingTaskSpawner>,
    waiting_point: WaitingPoint,
}

impl DriverInit {
    /// Creates initialization data for one driver instance.
    ///
    /// This constructor is intended for runtime implementations and driver tests.
    #[must_use]
    pub fn new(
        thread: Thread,
        blocking_tasks: Arc<dyn BlockingTaskSpawner>,
        waiting_point: WaitingPoint,
    ) -> Self {
        Self {
            thread,
            blocking_tasks,
            waiting_point,
        }
    }

    /// Returns the async worker this driver instance serves.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the runtime-owned facility for work that is allowed to block.
    #[must_use]
    pub fn blocking_tasks(&self) -> &dyn BlockingTaskSpawner {
        self.blocking_tasks.as_ref()
    }

    /// Returns whether this instance may provide the worker's waiting point.
    #[must_use]
    pub const fn waiting_point(&self) -> WaitingPoint {
        self.waiting_point
    }
}

impl fmt::Debug for DriverInit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverInit")
            .field("thread", &self.thread)
            .field("waiting_point", &self.waiting_point)
            .finish_non_exhaustive()
    }
}
