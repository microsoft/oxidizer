// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Poll;

use thread_aware::{Thread, ThreadAware};

use super::registry::TypedHook;

/// Wraps each top-level poll of an async task.
///
/// The handler runs on one async worker. Arty relocates it to that worker
/// before the task can run. The handler must call `task` exactly once and
/// return its result.
///
/// The scope includes the first poll, wake-driven polls, completion, and task
/// panics. It excludes blocking functions and nested future polls inside one
/// top-level task poll.
pub trait TaskPoll: ThreadAware + 'static {
    /// Runs one task poll inside this handler's scope.
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()>;
}

/// Reports that Arty registered an async task.
///
/// Arty creates this event on the selected worker before the task factory can
/// run. It covers tasks submitted through
/// [`RuntimeScheduler`](crate::task::RuntimeScheduler) and
/// [`Scheduler`](crate::task::Scheduler), including worker-specific and
/// relocating submissions. Arty does not create this event for a task that it
/// rejects before registration.
#[derive(derive_more::Debug)]
pub struct OnSpawn {
    pub(super) thread: Thread,
    #[debug(ignore)]
    pub(super) handlers: arty_executor::TaskPollHooks,
}

impl OnSpawn {
    pub(crate) fn new(thread: Thread) -> Self {
        Self {
            thread,
            handlers: arty_executor::TaskPollHooks::default(),
        }
    }

    pub(crate) fn into_handlers(self) -> arty_executor::TaskPollHooks {
        self.handlers
    }

    /// Returns the coordinate of the worker that will own the task.
    #[must_use]
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Adds a poll handler to the task.
    ///
    /// The first registered handler is the outermost handler. Arty relocates
    /// `handler` to the selected worker before the task can run. The first
    /// handler allocates only its boxed trait object. It does not allocate a
    /// separate collection buffer.
    pub fn register<H: TaskPoll>(&mut self, mut handler: H) {
        handler.relocate(Some(&self.thread), &self.thread);
        self.handlers.push(Box::new(TaskPollAdapter(handler)));
    }
}

struct TaskPollAdapter<H>(H);

impl<H: TaskPoll> arty_executor::TaskPoll for TaskPollAdapter<H> {
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        self.0.poll(task)
    }
}

#[derive(Clone, derive_more::Debug)]
pub(crate) struct SpawnHook(TypedHook<OnSpawn>);

impl SpawnHook {
    pub(crate) fn new<D>(data: D, callback: fn(&mut OnSpawn, &mut D)) -> Self
    where
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        Self(TypedHook::new(data, callback))
    }
    pub(crate) fn execute(&mut self, event: &mut OnSpawn) {
        self.0.execute(event);
    }
}

impl ThreadAware for SpawnHook {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.0.relocate(source, destination);
    }
}
