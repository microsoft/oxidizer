// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use crate::once_event::isolated::new_inefficient;
use crate::{LocalJoinHandle, LocalTaskMeta, SpawnQueue};

/// Allows the caller to schedule local async tasks that execute on the same thread as
/// an existing async task whose context provides this scheduler.
///
/// Because the tasks are scheduled
/// on the current thread, it's possible to capture `!Send` types. For this reason, `LocalTaskScheduler`
/// does not provide independent access to the thread state - it's assumed that the caller can capture
/// what is needed.
///
#[doc = include_str!("../../doc/snippets/async_task.md")]
///
/// Task scheduling is a multi-step process:
///
/// 1. (Optional) Define the task metadata via [`LocalTaskMeta`].
///    This allows you to name the task for diagnostic purposes and specify optional parameters for
///    advanced control over task scheduling.
/// 2. Prepare the task by calling `.prepare()` on the scheduler. This is
///    an asynchronous operation that allocates the runtime resources required to execute the task.
/// 3. Spawn the task by calling `.spawn()` on the object returned from
///    `.prepare()`. The new task will now start executing independently
///    from the task that scheduled it.
/// 4. (Optional) Await the returned join handle to obtain the result of the task.
#[derive(Debug, Clone)]
pub struct LocalTaskScheduler {
    // We use a weak reference here because tasks enqueued in the spawn queue can hold a reference
    // to the local scheduler, creating a cycle.
    spawn_local: Weak<SpawnQueue>,
}

impl LocalTaskScheduler {
    pub(crate) const fn new(spawn_local: Weak<SpawnQueue>) -> Self {
        Self { spawn_local }
    }

    /// Starts a new local task.
    ///
    #[doc = include_str!("../../doc/snippets/local_task.md")]
    ///
    #[doc = include_str!("../../doc/snippets/task_future_factory.md")]
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> LocalJoinHandle<R>
    where
        FF: FnOnce() -> F + 'static,
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        self.spawn_with_meta(LocalTaskMeta::default(), future_factory)
    }

    /// Starts a new local task using the specified metadata.
    ///
    #[doc = include_str!("../../doc/snippets/local_task.md")]
    ///
    #[doc = include_str!("../../doc/snippets/task_future_factory.md")]
    pub fn spawn_with_meta<FF, F, R>(
        &self,
        _meta: impl Into<LocalTaskMeta>,
        future_factory: FF,
    ) -> LocalJoinHandle<R>
    where
        FF: FnOnce() -> F + 'static,
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        self.spawn_local.upgrade().map_or_else(
            || {
                // In case the weak reference is invalid, we're shutting down anyway, so we return
                // a local join handle with a dropped sender.
                let (_, rx) = new_inefficient();
                LocalJoinHandle::new(rx, Rc::new(Cell::new(false)))
            },
            |s| s.spawn_local(future_factory),
        )
    }
}