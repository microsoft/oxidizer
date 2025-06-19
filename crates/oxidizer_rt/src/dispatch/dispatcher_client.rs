// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::sync::Arc;

use crate::{
    Dispatch, DispatchStop, DispatcherCore, Instantiation, Placement, RemoteJoinHandle,
    RuntimeThreadState, SpawnInstance, ThreadWaiter,
};

/// Each task is given an `Rc` of a dispatcher client that can be used to send commands to the
/// dispatcher. Different workers use different dispatcher clients, with configuration suitable
/// for a given worker.
///
/// In its current incarnation, this client can be freely cloned (one clone for each worker)
/// and holds currently necessary shared mutable state in atomics.
/// Obviously non-performant and a placeholder dummy implementation.
pub struct DispatcherClient<TS> {
    pub(crate) core: Arc<DispatcherCore<ThreadWaiter, TS>>,
}

impl<TS> DispatcherClient<TS>
where
    TS: RuntimeThreadState,
{
    pub const fn new(core: Arc<DispatcherCore<ThreadWaiter, TS>>) -> Self {
        Self { core }
    }
}

impl<TS> Debug for DispatcherClient<TS> {
    #[cfg_attr(test, mutants::skip)] // Debug formatting not tested
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherClient").finish()
    }
}

// We want this to be clone even if RA is not clone since then we would need to propagate Clone bounds everywhere
// or put Clone as supertrait of RuntimeArgumentTypes (which we don't want to do since keeping RuntimeArgumentTypes
// dyn compatible is desirable).
impl<TS> Clone for DispatcherClient<TS> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
        }
    }
}

impl<TS> Dispatch for DispatcherClient<TS>
where
    TS: RuntimeThreadState,
{
    type ThreadState = TS;

    fn spawn<FF, F, R>(&self, placement: Placement, future_factory: FF) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core
            .spawn(placement, move |ts, _domain| future_factory(ts))
    }

    fn spawn_multiple<FF, F, R>(
        &self,
        placement: Placement,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(TS, SpawnInstance) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core
            .spawn_multiple(placement, instantiation, move |ts, si, _domain| {
                future_factory(ts, si)
            })
    }
}

impl<TS> DispatchStop for DispatcherClient<TS>
where
    TS: RuntimeThreadState,
{
    #[cfg_attr(test, mutants::skip)] // Will cause tests to hang due to runtime never stopping.
    fn stop(&self) {
        self.core.stop();
    }

    // Impractical to test real waiting at this API layer. We test the real waiter implementation
    // but not the API layers that simply call the waiter, as it is hard to prove the wait failed.
    #[cfg_attr(test, mutants::skip)]
    fn wait(&self) {
        let joiner = self.core.join();

        futures::executor::block_on(joiner);
    }
}