// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use crate::{RemoteJoinHandle, SystemTaskMeta, SystemWorker};

/// Allows the caller to schedule system tasks.
///
#[doc = include_str!("../../doc/snippets/system_task.md")]
///
/// Task scheduling is a multi-step process:
///
/// 1. (Optional) Define the task metadata via [`SystemTaskMeta`].
///    This allows you to name the task for diagnostic purposes and specify optional parameters for
///    advanced control over task scheduling.
/// 2. Prepare the task by calling `.prepare()` on the scheduler. This is
///    an asynchronous operation that allocates the runtime resources required to execute the task.
/// 3. Spawn the task by calling `.spawn()` on the object returned from
///    `.prepare()`. The new task will now start executing independently
///    of the task that scheduled it.
/// 4. (Optional) Await the returned join handle to obtain the result of the task.
#[derive(Debug, Clone)]
pub struct SystemTaskScheduler {
    system_worker: Rc<SystemWorker>,
}

impl SystemTaskScheduler {
    pub(crate) const fn new(system_worker: Rc<SystemWorker>) -> Self {
        Self { system_worker }
    }

    /// Starts a new system task.
    ///
    #[doc = include_str!("../../doc/snippets/system_task.md")]
    pub fn spawn<B, R>(&self, body: B) -> RemoteJoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_with_meta(SystemTaskMeta::default(), body)
    }

    /// Starts a new system task using the specified metadata.
    ///
    #[doc = include_str!("../../doc/snippets/system_task.md")]
    pub fn spawn_with_meta<B, R>(
        &self,
        meta: impl Into<SystemTaskMeta>,
        body: B,
    ) -> RemoteJoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.system_worker.spawn_system(meta.into(), body)
    }
}