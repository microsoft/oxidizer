// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use thread_aware_core::Thread;

use crate::{DriverHandle, SystemTaskSpawner};

/// Placement and runtime facilities supplied when one driver instance is created.
///
/// The options also expose drivers already registered on the same thread. Those drivers may be
/// thread-local, so these options are neither [`Send`] nor [`Sync`] and must remain on the thread
/// where the runtime constructs it.
pub struct DriverOptions<'a> {
    thread: Thread,
    spawner: SystemTaskSpawner,
    drivers: Vec<DriverHandle<'a>>,
}

impl<'a> DriverOptions<'a> {
    /// Creates the options for one driver instance.
    ///
    /// This constructor is intended for runtime implementations and driver tests.
    #[must_use]
    pub fn new(thread: Thread, spawner: SystemTaskSpawner, drivers: Vec<DriverHandle<'a>>) -> Self {
        Self { thread, spawner, drivers }
    }

    /// Returns the async worker this driver instance serves.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the runtime-owned spawner for blocking I/O system work.
    #[must_use]
    pub const fn spawner(&self) -> &SystemTaskSpawner {
        &self.spawner
    }

    /// Returns drivers registered earlier on this thread, in runtime-defined order.
    ///
    /// Each type-erased handle is valid only for this creation call. A driver can inspect or
    /// downcast a handle and clone any independently owned state it needs to retain.
    #[must_use]
    pub fn drivers(&self) -> &[DriverHandle<'a>] {
        &self.drivers
    }
}

impl fmt::Debug for DriverOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DriverOptions")
            .field("thread", &self.thread)
            .field("driver_count", &self.drivers.len())
            .finish_non_exhaustive()
    }
}
