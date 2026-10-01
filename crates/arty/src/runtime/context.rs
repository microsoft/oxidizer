// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use arty_executor::TaskSet;
use many_cpus::ProcessorSet;
use observed::Sink;
use thread_aware::Thread;
use tick::Clock;

use crate::runtime::dispatch::DispatcherClient;
use crate::task::builtins::InnerBuiltins;
use crate::task::local::LocalTaskBinding;
use crate::task::scheduler::TaskScheduler;

/// Runtime-local slots sharing the dispatcher's worker-index ordering. Each worker publishes
/// its immutable service bundle once during startup; relocation selects the corresponding
/// destination slot without taking a shared lock.
pub(crate) type SharedState = Arc<[OnceLock<Arc<InnerBuiltins>>]>;

/// Capabilities assembled during worker initialization.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct RuntimeBuiltins {
    pub(crate) core: CoreRuntimeBuiltins,

    pub(crate) task_scheduler: TaskScheduler,

    pub(crate) clock: Clock,
}

impl RuntimeBuiltins {
    pub(in crate::runtime) fn new(dispatcher: &Rc<DispatcherClient>, core: CoreRuntimeBuiltins, clock: Clock, thread: Thread) -> Self {
        Self {
            core,
            task_scheduler: TaskScheduler::new(dispatcher.as_ref().clone(), thread),
            clock,
        }
    }
}

/// Separate structure for runtime capabilities that are independent of the thread state.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub(crate) struct CoreRuntimeBuiltins {
    pub(crate) local_scheduler: LocalTaskBinding,
    pub(crate) processor_set: ProcessorSet,
    pub(crate) thread: Thread,

    pub(crate) sink: Sink,
}

impl CoreRuntimeBuiltins {
    pub(in crate::runtime) fn new(
        tasks: TaskSet,
        dispatcher: DispatcherClient,
        thread: Thread,
        processor_set: ProcessorSet,
        sink: Sink,
    ) -> Self {
        Self {
            local_scheduler: LocalTaskBinding::new(tasks, sink.clone(), dispatcher.shutdown_signal()),
            processor_set,
            thread,
            sink,
        }
    }
}
