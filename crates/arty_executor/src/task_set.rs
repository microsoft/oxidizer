// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;
use std::rc::{Rc, Weak};

use crate::inspection::InspectWith;
use crate::{ExecutorCore, InspectionError, InspectionStatus, JoinHandle, PollObserver, TaskState};

/// Represents the set of tasks currently registered with an executor and allows new tasks
/// to be registered.
///
/// The task set acts like a handle and can be cheaply cloned and used from anywhere on the same
/// thread as the executor, including from inside the futures of tasks running on the same executor.
/// Every clone is functionally identical.
#[derive(Clone, Debug)]
pub struct TaskSet {
    // We only keep a weak reference, so even if the owner of this type does something odd, we do
    // not extend the lifetime of the `Executor`, which is governed by the `Executor` owner.
    core: Weak<ExecutorCore>,
}

impl TaskSet {
    /// Installs one shared, worker-local poll observer between execution cycles.
    ///
    /// The shared ownership allows the runtime to read collected measurements
    /// from its inspection callback without storing observer handles in tasks.
    ///
    /// # Errors
    ///
    /// Returns an error if the executor is executing or has been dropped.
    pub fn set_poll_observer(&self, observer: Rc<dyn PollObserver>) -> Result<(), InspectionError> {
        self.core
            .upgrade()
            .ok_or(InspectionError::Disconnected)?
            .set_poll_observer(observer)
    }

    /// Registers a future with an on-demand description, stored in the same task allocation.
    ///
    /// The description function is not called on spawn, wake, poll, or completion.
    /// It is called by [`Self::inspect`] or an enabled [`PollObserver`].
    ///
    /// # Panics
    ///
    /// Panics if this executor has been dropped or is shutting down.
    pub fn add_with_inspection<F, R, D, I>(&self, future: F, inspect: I) -> JoinHandle<R>
    where
        F: Future<Output = R> + 'static,
        R: 'static,
        D: 'static,
        I: Fn(&F, TaskState) -> Option<D> + 'static,
    {
        self.core
            .upgrade()
            .expect("task set is disconnected - executor has been dropped")
            .add_inspected_task(future, InspectWith::new(inspect))
    }

    /// Visits typed descriptions of at most `limit` live tasks without consuming notifications.
    ///
    /// Completed tasks are excluded. Tasks without descriptions and descriptions of other types
    /// are omitted, but still count toward the work limit. Invoke this between execution cycles.
    /// The visitor must not add tasks or recursively inspect this executor.
    ///
    /// # Errors
    ///
    /// Returns an error when the executor is busy, has been dropped, or a
    /// description function or visitor panics.
    pub fn inspect<D: 'static>(&self, limit: NonZeroUsize, mut visitor: impl FnMut(&D)) -> Result<InspectionStatus, InspectionError> {
        let core = self.core.upgrade().ok_or(InspectionError::Disconnected)?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            core.inspect(limit, &mut |data| {
                if let Some(data) = data.downcast_ref::<D>() {
                    visitor(data);
                }
            })
        }))
        .unwrap_or(Err(InspectionError::Panicked))
    }

    pub(crate) fn new(core: &Rc<ExecutorCore>) -> Self {
        Self { core: Rc::downgrade(core) }
    }

    /// Registers a future to be processed by the executor as a new task.
    ///
    /// The future will be polled until it completes, after which it will be dropped, unless the
    /// executor is commanded to shut down, in which case the future may be dropped before it
    /// completes.
    ///
    /// # Panics
    ///
    /// Panics if the executor is already shutting down. This method will never panic when
    /// called from inside an existing task on the same executor (the task is running,
    /// therefore the executor is not shutting down).
    pub fn add<F, R>(&self, future: F) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        self.core
            .upgrade()
            .expect("task set is disconnected - executor has been dropped")
            .add_task(future)
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(TaskSet: Send, Sync);
    }
}
