// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Weak;

use mockall::mock;

use crate::{
    CoreRuntimeBuiltins, Dispatch, DispatchStop, Instantiation, LocalTaskScheduler,
    MultiInstanceFutureFactory, Placement, RemoteJoinHandle, RuntimeBuiltins, RuntimeThreadState,
    SpawnQueue,
};

impl RuntimeThreadState for TestTaskContext {
    type SharedState = ();
    type Error = ();
    type SharedInitState = ();

    async fn async_init(
        _shared_state: &Self::SharedState,
        _builtins: CoreRuntimeBuiltins,
    ) -> Result<Self::SharedInitState, Self::Error> {
        Ok(()) // No async initialization needed for TestTaskContext
    }

    fn sync_init(
        _shared_state: &Self::SharedState,
        _shared_init_state: Self::SharedInitState,
        builtins: RuntimeBuiltins<Self>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            local_task_scheduler: builtins.core.local_scheduler,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TestTaskContext {
    // We no longer have a dispatcher here, but that should be fine as we can get it from a TaskScheduler in the future if a test needs it.
    pub local_task_scheduler: LocalTaskScheduler,
}

impl TestTaskContext {
    pub fn new(queue: Weak<SpawnQueue>) -> Self {
        Self {
            local_task_scheduler: LocalTaskScheduler::new(queue),
        }
    }
}

mock! {
    #[derive(Debug)]
    pub Dispatcher {}

    impl Dispatch for Dispatcher {
        type ThreadState = TestTaskContext;

        fn spawn<FF, F, R>(&self, placement: Placement, future_factory: FF) -> RemoteJoinHandle<R>
        where
            FF: FnOnce(TestTaskContext) -> F + Send + 'static,
            F: Future<Output = R> + 'static,
            R: Send + 'static;

        fn spawn_multiple<FF, F, R>(
            &self,
            placement: Placement,
            instantiation: Instantiation,
            future_factory: FF,
        ) -> Box<[RemoteJoinHandle<R>]>
        where
            FF: MultiInstanceFutureFactory<TestTaskContext, F, R>,
            F: Future<Output = R> + 'static,
            R: Send + 'static;
    }

    impl DispatchStop for Dispatcher {
        fn stop(&self);
        fn wait(&self);
    }

    impl Clone for Dispatcher {
        fn clone(&self) -> Self;
    }
}