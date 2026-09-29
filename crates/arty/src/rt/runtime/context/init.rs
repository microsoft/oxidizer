// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use arty_executor::TaskSet;
use many_cpus::ProcessorSet;
use observed::Sink;
use thread_aware::Thread;
use tick::Clock;

use crate::rt::runtime::context::InnerBuiltins;
use crate::rt::runtime::dispatch::DispatcherClient;
use crate::rt::task::local::LocalTaskBinding;
use crate::rt::task::scheduler::TaskScheduler;

/// Runtime-local slots sharing the dispatcher's worker-index ordering. Each worker publishes
/// its immutable service bundle once during startup; relocation selects the corresponding
/// destination slot without taking a shared lock.
pub(in crate::rt::runtime) type SharedState = Arc<[OnceLock<Arc<InnerBuiltins>>]>;

/// This trait should be implemented by types that can be used as per-thread state in the runtime.
///
/// One value is created for each worker and cloned for each task.
pub(in crate::rt::runtime) trait RuntimeThreadState: Clone + 'static {
    /// The type of shared state that is needed to create the per-thread value. Each `init` call gets a reference
    /// to a shared value of this type.
    type SharedState: Send + Sync + 'static;

    /// The type of error that can be returned from the initialization phases.
    type Error: Debug + Send + 'static;

    /// Called once per worker. Tasks receive an owned clone of the resulting value.
    ///
    /// # Deadlock risk
    ///
    /// Do not block on futures in this function. Otherwise, you're risking a deadlock during initialization. The specific requirement is
    /// that you cannot block on a task spawned using the `TaskScheduler<Self>`, but avoid blocking in general as the specific requirement
    /// may change in the future.
    fn sync_init(shared_state: &Self::SharedState, builtins: RuntimeBuiltins) -> Result<Self, Self::Error>;
}

/// Capabilities assembled during worker initialization.
#[derive(Debug)]
#[non_exhaustive]
pub(in crate::rt::runtime) struct RuntimeBuiltins {
    pub(in crate::rt::runtime) core: CoreRuntimeBuiltins,

    pub(super) task_scheduler: TaskScheduler,

    pub(super) clock: Clock,
}

impl RuntimeBuiltins {
    pub(in crate::rt::runtime) fn new(dispatcher: &Rc<DispatcherClient>, core: CoreRuntimeBuiltins, clock: Clock, thread: Thread) -> Self {
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
pub(in crate::rt::runtime) struct CoreRuntimeBuiltins {
    pub(in crate::rt::runtime) local_scheduler: LocalTaskBinding,
    pub(super) dispatcher: DispatcherClient,
    pub(super) processor_set: ProcessorSet,
    pub(super) thread: Thread,

    pub(super) sink: Sink,
}

impl CoreRuntimeBuiltins {
    pub(in crate::rt::runtime) fn new(
        tasks: TaskSet,
        dispatcher: DispatcherClient,
        thread: Thread,
        processor_set: ProcessorSet,
        sink: Sink,
    ) -> Self {
        Self {
            local_scheduler: LocalTaskBinding::new(tasks, sink.clone()),
            dispatcher,
            processor_set,
            thread,
            sink,
        }
    }
}
