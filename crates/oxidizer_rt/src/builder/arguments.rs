// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;
use std::{fmt::Debug, sync::Arc};

use isolated_domains::Domain;
use oxidizer_time::Clock;

use crate::{
    DispatchStop, DispatcherClient, IsolatedTaskScheduler, LocalTaskScheduler, ProcessorSet,
    RuntimeOperations, SpawnQueue, SystemTaskScheduler, SystemWorker, TaskScheduler,
    TaskSchedulerCore,
};

/// This trait should be implemented by types that can be used as per-thread state in the runtime.
///
/// This means a value of this type will be created for each thread participating in the runtime. A clone
/// of this value will then be provided to each task scheduled. For this reason, cloning of this value
/// should be cheap.
pub trait RuntimeThreadState: Clone + 'static {
    /// The type of shared state that is needed to create the per-thread value. Each `init` call gets a reference
    /// to a shared value of this type.
    type SharedState: Send + Sync + 'static;

    /// The type of error that can be returned from the initialization phases.
    type Error: Debug + Send + 'static;

    /// State that is shared between the async and sync initialization phases. This is used to pass data
    /// from the async initialization phase to the sync initialization phase.
    type SharedInitState;

    /// This function will get called **once** for each thread participating in the runtime. The resulting
    /// value will then be passed along to the `sync_init` function. This function is async and can be used
    /// to perform any async initialization needed for the thread state. This function will be called
    /// before the `sync_init` function, so any async initialization needed for the thread state should
    /// be done here. This function will be called in the context of the thread that is being initialized.
    ///
    /// # Deadlock risk
    /// Do not block on futures in this function. Otherwise, you're risking a deadlock during initialization.
    fn async_init(
        shared_state: &Self::SharedState,
        builtins: CoreRuntimeBuiltins,
    ) -> impl Future<Output = Result<Self::SharedInitState, Self::Error>>;

    /// This function will get called **once** for each thread participating in the runtime. The resulting value will then be cloned for each
    /// spawned task. This means that this function can be relatively expensive computationally, but the Clone implementation of the resulting
    /// value shouldn't be. Ideally, the resulting value would be passed to each task by reference, but there are lifetime issues associated
    /// with that, so it probably won't be possible anytime soon.
    ///
    /// # Deadlock risk
    ///
    /// Do not block on futures in this function. Otherwise, you're risking a deadlock during initialization. The specific requirement is
    /// that you cannot block on a task spawned using the `TaskScheduler<Self>`, but avoid blocking in general as the specific requirement
    /// may change in the future.
    fn sync_init(
        shared_state: &Self::SharedState,
        shared_init_state: Self::SharedInitState,
        builtins: RuntimeBuiltins<Self>,
    ) -> Result<Self, Self::Error>;
}

/// Capabilities provided by Runtime. See the documentation of the types of the fields for more information
/// This is non-exhaustive to allow Oxidizer to add more runtime capabilities in the future.
#[derive(Debug)]
#[non_exhaustive]
pub struct RuntimeBuiltins<TS>
where
    TS: RuntimeThreadState,
{
    pub task_scheduler: TaskScheduler<'static, TS>,
    pub core: CoreRuntimeBuiltins,

    // TODO: There are currently two schedulers; `TaskScheduler` will be removed in the future.
    pub isolated_task_scheduler: IsolatedTaskScheduler<TS>,
}

impl<TS> RuntimeBuiltins<TS>
where
    TS: RuntimeThreadState,
{
    pub(crate) fn new(
        dispatcher: &Rc<DispatcherClient<TS>>,
        core: CoreRuntimeBuiltins,
        domain: Domain,
    ) -> Self {
        Self {
            task_scheduler: TaskScheduler::new(TaskSchedulerCore::new(Rc::clone(dispatcher))),
            core,
            isolated_task_scheduler: IsolatedTaskScheduler::new(
                Arc::clone(&dispatcher.core),
                domain,
            ),
        }
    }
}

/// Separate structure for runtime capabilities that are independent of the thread state.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CoreRuntimeBuiltins {
    pub local_scheduler: LocalTaskScheduler,
    pub system_scheduler: SystemTaskScheduler,
    pub runtime_ops: RuntimeOperations,
    pub clock: Clock,
    pub processor_set: ProcessorSet,

    /// The IO context is used to schedule IO tasks and manage IO resources.
    ///
    /// Currently, the `Context` only works on Windows. On other platforms, any operations that
    /// require the `Context` will panic.
    pub io_context: oxidizer_io::Context,
}

impl CoreRuntimeBuiltins {
    #[allow(
        clippy::too_many_arguments,
        reason = "Internal API, not exposed to users"
    )]
    pub(crate) fn new(
        clock: Clock,
        queue: &Rc<SpawnQueue>,
        system_worker: &Rc<SystemWorker>,
        dispatcher: Rc<dyn DispatchStop>,
        domain: Domain,
        processor_set: ProcessorSet,
        io_context: oxidizer_io::Context,
    ) -> Self {
        Self {
            local_scheduler: LocalTaskScheduler::new(Rc::downgrade(queue)),
            system_scheduler: SystemTaskScheduler::new(Rc::clone(system_worker)),
            runtime_ops: RuntimeOperations::new(dispatcher, Some(domain)),
            clock,
            processor_set,
            io_context,
        }
    }
}

/// A basic implementation of `ThreadState` that simply provides the runtime capabilities as is. Can be
/// useful for simple cases where no additional state is needed (e.g. in some tests)
#[derive(Debug, Clone)]
pub struct BasicThreadState {
    builtins: Rc<RuntimeBuiltins<BasicThreadState>>,
}

impl BasicThreadState {
    /// Get the runtime builtins reference.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn builtins(&self) -> &RuntimeBuiltins<Self> {
        &self.builtins
    }

    /// Shortcut to access the `Clock`.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn clock(&self) -> &Clock {
        &self.builtins.core.clock
    }

    /// Shortcut to access `RuntimeOperations`
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn runtime_ops(&self) -> &RuntimeOperations {
        &self.builtins.core.runtime_ops
    }

    /// Shortcut to access `TaskScheduler`
    #[must_use]
    pub fn scheduler(&self) -> TaskScheduler<Self> {
        self.builtins.task_scheduler.borrowed()
    }

    /// Shortcut to access `LocalTaskScheduler`
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn local_scheduler(&self) -> &LocalTaskScheduler {
        &self.builtins.core.local_scheduler
    }

    /// Shortcut to access `SystemTaskScheduler`
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn system_scheduler(&self) -> &SystemTaskScheduler {
        &self.builtins.core.system_scheduler
    }

    /// Shortcut to access the [`oxidizer_io::Context`].
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn io_context(&self) -> &oxidizer_io::Context {
        &self.builtins.core.io_context
    }
}

impl RuntimeThreadState for BasicThreadState {
    type SharedState = ();
    type Error = BasicThreadStateError;
    type SharedInitState = ();

    #[cfg_attr(test, mutants::skip)]
    async fn async_init(
        _shared_state: &Self::SharedState,
        _builtins: CoreRuntimeBuiltins,
    ) -> Result<Self::SharedInitState, Self::Error> {
        // No async initialization needed for BasicThreadState
        Ok(())
    }

    fn sync_init(
        _shared_state: &Self::SharedState,
        _shared_init_state: Self::SharedInitState,
        builtins: RuntimeBuiltins<Self>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            builtins: Rc::new(builtins),
        })
    }
}

/// Error type for `BasicThreadState`.
///
/// Since basic thread  state is a simple wrapper around the
/// `RuntimeBuiltins` and cannot fail initialization, this error type is empty. It is only used to
/// make sure the Error type implements `std::error::Error` and can be used with crates like `anyhow`.
#[derive(Debug, Clone)]
pub struct BasicThreadStateError;

impl std::error::Error for BasicThreadStateError {}

impl std::fmt::Display for BasicThreadStateError {
    #[cfg_attr(test, mutants::skip)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BasicThreadStateError")
    }
}