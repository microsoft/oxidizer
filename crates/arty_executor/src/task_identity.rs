// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::panic::Location;

use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};

thread_local! {
    static CURRENT: Cell<Option<TaskIdentity>> = const { Cell::new(None) };
}

/// Identifies one task and the source location used to create its identity.
///
/// Pass this identity to [`TaskSet::task`](crate::TaskSet::task) when registering
/// a task. The executor makes it available through [`Self::current`] during each
/// poll, whether or not task monitoring is enabled.
/// See [`TaskSet::task`](crate::TaskSet::task) for a registration example.
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
            id: seismograph_runtime::allocate_task_id(),
            descriptor: TypeDescriptorId::from_raw(std::ptr::from_ref(location).addr() as u64)
                .expect("a static location reference has a nonzero address"),
            location,
        }
    }

    /// Returns the identity of the task currently running on this thread.
    ///
    /// Returns `None` outside executor polls.
    /// This state is local to this version of `arty_executor`. It does not flow
    /// to another thread.
    #[inline]
    #[must_use]
    pub fn current() -> Option<Self> {
        CURRENT.with(Cell::get)
    }

    pub(crate) fn scope<R>(self, body: impl FnOnce() -> R) -> R {
        let _current = CurrentTask(CURRENT.with(|slot| slot.replace(Some(self))));
        body()
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

struct CurrentTask(Option<TaskIdentity>);

impl Drop for CurrentTask {
    fn drop(&mut self) {
        CURRENT.with(|slot| slot.set(self.0));
    }
}
