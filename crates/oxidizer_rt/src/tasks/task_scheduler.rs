// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::{
    DispatcherClient, Instantiation, RemoteJoinHandle, RuntimeThreadState, SpawnInstance, TaskMeta,
    TaskSchedulerCore,
};

/// Allows the caller to schedule async tasks. These tasks have access to the thread state.
/// Consider using `LocalTaskScheduler` if you don't need to load-balance tasks across threads.
///
#[doc = include_str!("../../doc/snippets/async_task.md")]
///
/// Task scheduling is a multi-step process:
///
/// 1. (Optional) Define the task metadata via [`TaskMeta`].
///    This allows you to name the task for diagnostic purposes and specify optional parameters for
///    advanced control over task scheduling.
/// 2. Prepare the task by calling `.prepare()` or `.prepare_multiple()` on the scheduler. This is
///    an asynchronous operation that allocates the runtime resources required to execute the task.
/// 3. Spawn the task by calling `.spawn()` or `.spawn_multiple()` on the object returned from
///    `.prepare()` or `.prepare_multiple()`. The new task will now start executing independently
///    from the task that scheduled it.
/// 4. (Optional) Await the returned join handle to obtain the result of the task.
#[derive(Debug)]
pub struct TaskScheduler<'cx, TS> {
    core: Cow<'cx, TaskSchedulerCore<DispatcherClient<TS>>>,
}

impl<TS> TaskScheduler<'_, TS>
where
    TS: RuntimeThreadState,
{
    pub(crate) const fn new(
        core: TaskSchedulerCore<DispatcherClient<TS>>,
    ) -> TaskScheduler<'static, TS> {
        TaskScheduler {
            core: Cow::Owned(core),
        }
    }

    /// Starts a new async task.
    ///
    #[doc = include_str!("../../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../../doc/snippets/task_future_factory.md")]
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.spawn_with_meta(TaskMeta::default(), future_factory)
    }

    /// Starts a new async task using the specified metadata.
    ///
    #[doc = include_str!("../../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../../doc/snippets/task_future_factory.md")]
    pub fn spawn_with_meta<FF, F, R>(
        &self,
        meta: impl Into<TaskMeta>,
        future_factory: FF,
    ) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core.spawn_with_meta(&(meta.into()), future_factory)
    }

    /// Starts multiple async tasks.
    ///
    #[doc = include_str!("../../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../../doc/snippets/task_future_factory.md")]
    pub fn spawn_multiple<FF, F, R>(
        &self,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(TS, SpawnInstance) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.spawn_multiple_with_meta(instantiation, TaskMeta::default(), future_factory)
    }

    /// Starts multiple async tasks using the specified metadata.
    ///
    #[doc = include_str!("../../doc/snippets/async_task.md")]
    ///
    #[doc = include_str!("../../doc/snippets/task_future_factory.md")]
    pub fn spawn_multiple_with_meta<FF, F, R>(
        &self,
        instantiation: Instantiation,
        meta: impl Into<TaskMeta>,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(TS, SpawnInstance) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core
            .spawn_multiple_with_meta(instantiation, &(meta.into()), future_factory)
    }

    /// Detaches the scheduler from the context of the current task, allowing it to be used
    /// at an arbitrary moment in time on the current thread, during execution of any task.
    #[must_use]
    pub fn detach(&self) -> TaskScheduler<'static, TS> {
        TaskScheduler {
            core: Cow::Owned((*self.core).clone()),
        }
    }

    #[doc = include_str!("../../doc/snippets/scheduler_with_lifetime.md")]
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn borrowed(&self) -> TaskScheduler<'_, TS> {
        TaskScheduler {
            core: Cow::Borrowed(&self.core),
        }
    }
}