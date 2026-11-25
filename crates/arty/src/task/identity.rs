// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::Location;

use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};

/// Identifies one task and the source location used to create its identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TaskIdentity {
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
    pub(crate) fn from_location(location: &'static Location<'static>) -> Self {
        Self {
            id: seismograph_runtime::allocate_task_id(),
            descriptor: TypeDescriptorId::from_raw(std::ptr::from_ref(location).addr() as u64)
                .expect("a static location reference has a nonzero address"),
            location,
        }
    }

    pub(crate) const fn id(self) -> TaskId {
        self.id
    }

    pub(crate) const fn descriptor(self) -> TypeDescriptorId {
        self.descriptor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_location_creates_distinct_tasks_with_its_address_as_descriptor() {
        let location = Location::caller();
        let first = TaskIdentity::from_location(location);
        let second = TaskIdentity::from_location(location);

        assert_ne!(first.id, second.id);
        assert_eq!(first.descriptor, second.descriptor);
        assert_eq!(first.descriptor.get(), std::ptr::from_ref(location).addr() as u64);
        assert!(std::ptr::eq(first.location, location));
    }

    #[test]
    fn different_locations_create_distinct_descriptors() {
        let first = TaskIdentity::from_location(Location::caller());
        let second = TaskIdentity::from_location(Location::caller());

        assert_ne!(first.descriptor, second.descriptor);
    }
}
