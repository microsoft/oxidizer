// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::Location;

use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};

/// Identifies one task and the source location used to create its identity.
///
/// A runtime can retain this identity independently of monitoring and use it
/// for task inspection and lifecycle events.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskIdentity {
    id: TaskId,
    descriptor: TypeDescriptorId,
    location: &'static Location<'static>,
}

impl TaskIdentity {
    /// Creates a new task ID and a descriptor from the static location's address.
    ///
    /// Each call creates a distinct task ID. Calls with the same location
    /// reference share a descriptor ID. This does not start or register a task.
    ///
    /// # Panics
    ///
    /// Panics if process task IDs are exhausted.
    #[must_use]
    pub fn from_location(location: &'static Location<'static>) -> Self {
        Self {
            id: crate::runtime::allocate_task_id(),
            descriptor: TypeDescriptorId::from_raw(std::ptr::from_ref(location).addr() as u64)
                .expect("a static location reference has a nonzero address"),
            location,
        }
    }

    /// Returns the process-wide task ID.
    #[inline]
    #[must_use]
    pub const fn id(self) -> TaskId {
        self.id
    }

    /// Returns the descriptor ID derived from the source location's address.
    #[inline]
    #[must_use]
    pub const fn descriptor(self) -> TypeDescriptorId {
        self.descriptor
    }

    /// Returns the source location supplied when this identity was created.
    #[inline]
    #[must_use]
    pub const fn location(self) -> &'static Location<'static> {
        self.location
    }
}
