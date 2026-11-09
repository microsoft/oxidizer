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
    pub(super) handlers: Option<TaskPolls>,
}

impl OnSpawn {
    pub(crate) fn new(thread: Thread) -> Self {
        Self { thread, handlers: None }
    }

    pub(crate) fn into_handlers(self) -> Option<TaskPolls> {
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
        let handler = Box::new(handler) as Box<dyn TaskPoll>;
        if let Some(handlers) = &mut self.handlers {
            handlers.push(handler);
        } else {
            self.handlers = Some(TaskPolls::new(handler));
        }
    }
}

#[repr(transparent)]
#[derive(derive_more::Debug)]
pub(crate) struct TaskPolls(TaskPollChain);

impl TaskPolls {
    fn new(first: Box<dyn TaskPoll>) -> Self {
        Self(TaskPollChain {
            first,
            additional: Vec::new(),
        })
    }

    fn push(&mut self, handler: Box<dyn TaskPoll>) {
        self.0.additional.push(handler);
    }
}

#[derive(derive_more::Debug)]
struct TaskPollChain {
    #[debug(ignore)]
    first: Box<dyn TaskPoll>,
    #[debug(ignore)]
    additional: Vec<Box<dyn TaskPoll>>,
}

impl arty_executor::TaskPoll for TaskPolls {
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        fn poll_next(hooks: &mut [Box<dyn TaskPoll>], task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
            let Some((hook, remaining)) = hooks.split_first_mut() else {
                return task();
            };
            hook.poll(&mut || poll_next(remaining, task))
        }

        self.0.first.poll(&mut || poll_next(&mut self.0.additional, task))
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

#[cfg(all(test, not(miri)))]
mod tests {
    use std::alloc::System;
    use std::hint::black_box;

    use alloc_tracker::{Allocator, Session};
    use thread_aware::ThreadBuilder;

    use super::*;

    #[global_allocator]
    static ALLOCATOR: Allocator<System> = Allocator::system();

    const MEASURED_OPERATIONS: usize = 1_000;

    struct OneBytePoll(u8);

    impl ThreadAware for OneBytePoll {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl TaskPoll for OneBytePoll {
        fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
            black_box(self.0);
            task()
        }
    }

    #[test]
    fn one_registered_poll_uses_one_allocation() {
        let worker = ThreadBuilder::default().build(std::thread::current().id());
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("one_registered_poll");
        {
            let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
            for value in 0..MEASURED_OPERATIONS {
                let mut event = OnSpawn::new(worker.clone());
                event.register(OneBytePoll(value as u8));
                black_box(event.into_handlers());
            }
        }

        let allocations = session
            .to_report()
            .operations()
            .find_map(|(name, operation)| (name == "one_registered_poll").then(|| operation.total_allocations_count()))
            .unwrap();
        assert_eq!(
            allocations, MEASURED_OPERATIONS as u64,
            "one registered poll must allocate only its boxed trait object"
        );
    }
}
