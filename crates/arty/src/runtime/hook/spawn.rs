// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::{Thread, ThreadAware};

use super::hook_fn::ErasedHookFn;

/// Observes each top-level poll of an async task.
///
/// The handler runs on one async worker. Arty relocates it to that worker
/// before the task can run.
///
/// The scope includes the first poll, wake-driven polls, completion, and task
/// panics. It excludes blocking functions and nested future polls inside one
/// top-level task poll. Implementations must not panic. Arty does not catch
/// panics from these methods.
pub trait TaskPoll: ThreadAware + 'static {
    /// Runs immediately before Arty polls the task.
    fn begin(&mut self);

    /// Runs immediately after Arty polls the task.
    ///
    /// Arty also calls this method after it contains a panic from the task
    /// poll.
    fn end(&mut self);
}

/// Reports that Arty is submitting an async task.
///
/// Arty creates this event before it sends the task to its selected worker.
/// On a runtime worker, Arty uses that worker's hook instance. Outside runtime
/// workers, Arty uses a worker-neutral hook instance. The event covers tasks submitted through
/// [`RuntimeScheduler`](crate::task::RuntimeScheduler) and
/// [`Scheduler`](crate::task::Scheduler), including worker-specific and
/// relocating submissions. Arty does not create this event for a task that it
/// rejects before registration. The callback must not panic.
#[derive(derive_more::Debug)]
pub struct Spawn {
    pub(super) thread: Option<Thread>,
    #[debug(ignore)]
    pub(super) handlers: TaskPolls,
}

impl Spawn {
    pub(crate) fn new(thread: Option<Thread>) -> Self {
        Self {
            handlers: TaskPolls::new(thread.clone()),
            thread,
        }
    }

    pub(crate) fn into_handlers(self) -> TaskPolls {
        self.handlers
    }

    /// Returns the worker that submitted the task.
    ///
    /// Returns `None` when the submission originated outside a runtime worker.
    #[must_use]
    pub fn thread(&self) -> Option<&Thread> {
        self.thread.as_ref()
    }

    /// Adds a poll handler to the task.
    ///
    /// The first registered handler is the outermost handler. Arty relocates
    /// `handler` after the task factory creates the future on the selected
    /// worker. The first handler allocates only its boxed trait object. It does
    /// not allocate a separate collection buffer.
    pub fn register<H: TaskPoll>(&mut self, handler: H) {
        let handler = Box::new(handler) as Box<dyn TaskPoll>;
        self.handlers.push(handler);
    }
}

#[derive(derive_more::Debug)]
pub(crate) struct TaskPolls {
    source: Option<Thread>,
    chain: Option<TaskPollChain>,
}

impl TaskPolls {
    fn new(source: Option<Thread>) -> Self {
        Self { source, chain: None }
    }

    fn push(&mut self, handler: Box<dyn TaskPoll>) {
        if let Some(chain) = &mut self.chain {
            chain.additional.push(handler);
        } else {
            self.chain = Some(TaskPollChain {
                first: handler,
                additional: Vec::new(),
            });
        }
    }

    pub(crate) fn relocate_to(&mut self, destination: &Thread) {
        if let Some(chain) = &mut self.chain {
            chain.first.relocate(self.source.as_ref(), destination);
            for handler in &mut chain.additional {
                handler.relocate(self.source.as_ref(), destination);
            }
        }
        self.source = Some(destination.clone());
    }

    pub(crate) fn begin(&mut self) {
        let Some(chain) = &mut self.chain else {
            return;
        };
        chain.first.begin();
        for handler in &mut chain.additional {
            handler.begin();
        }
    }

    pub(crate) fn end(&mut self) {
        let Some(chain) = &mut self.chain else {
            return;
        };
        for handler in chain.additional.iter_mut().rev() {
            handler.end();
        }
        chain.first.end();
    }
}

impl arty_executor::TaskPoll for TaskPolls {
    fn begin(&mut self) {
        Self::begin(self);
    }

    fn end(&mut self) {
        Self::end(self);
    }
}

#[derive(derive_more::Debug)]
struct TaskPollChain {
    #[debug(ignore)]
    first: Box<dyn TaskPoll>,
    #[debug(ignore)]
    additional: Vec<Box<dyn TaskPoll>>,
}

impl super::Hook for Spawn {}

impl super::private::Sealed for Spawn {
    type Erased<D>
        = ErasedHookFn<Self>
    where
        D: ThreadAware + Clone + 'static;
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
        fn begin(&mut self) {
            black_box(self.0);
        }

        fn end(&mut self) {}
    }

    #[test]
    fn one_registered_poll_uses_one_allocation() {
        let worker = ThreadBuilder::default().build(std::thread::current().id());
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("one_registered_poll");
        {
            let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
            for value in 0..MEASURED_OPERATIONS {
                let mut event = Spawn::new(Some(worker.clone()));
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
