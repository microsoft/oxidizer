// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::{
    BasicThreadState, DispatcherCore, Instantiation, PlacementToken, RemoteJoinHandle,
    RuntimeThreadState, TaskMeta, ThreadWaiter, meta_builders::TaskMetaBuilder,
};
use isolated_domains::{Domain, Transfer, TransferFnOnce};

#[derive(Debug, Clone)]
pub struct IsolatedTaskScheduler<TS = BasicThreadState> {
    dispatcher: Arc<DispatcherCore<ThreadWaiter, TS>>,
    current: Domain,
}

impl<TS> IsolatedTaskScheduler<TS>
where
    TS: RuntimeThreadState,
{
    pub(crate) fn new(dispatcher: Arc<DispatcherCore<ThreadWaiter, TS>>, current: Domain) -> Self {
        Self {
            dispatcher,
            current,
        }
    }

    /// Spawns a new task on the current domain of this scheduler, without transferring the closure.
    ///
    /// This still implies `Send` because it's possible that the scheduler has been moved to another thread.
    pub fn spawn_local<FF, F, R>(&self, future_factory: FF) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(TS) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.dispatcher.spawn(
            super::Placement::SameThreadAs(PlacementToken::new(self.current)),
            move |ts, _domain| future_factory(ts),
        )
    }

    /// Spawns a new task on the current domain of this scheduler.
    pub fn spawn<FF, F, R>(&self, f: FF) -> RemoteJoinHandle<R>
    where
        FF: TransferFnOnce<F> + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.spawn_with_meta(
            TaskMetaBuilder::new().placement(super::Placement::SameThreadAs(PlacementToken::new(
                self.current,
            ))),
            f,
        )
    }

    /// Spawns a new task, given the chosen placement.
    pub fn spawn_with_meta<FF, F, R>(
        &self,
        meta: impl Into<TaskMeta>,
        future_factory: FF,
    ) -> RemoteJoinHandle<R>
    where
        FF: TransferFnOnce<F> + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let source = self.current;
        self.dispatcher
            .spawn(meta.into().placement, async move |_, destination| {
                future_factory
                    .transfer(source, destination)
                    .await
                    .call_once()
                    .await
            })
    }

    pub fn spawn_multiple<FF, F, R>(
        &self,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: TransferFnOnce<F> + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.spawn_multiple_with_meta(
            TaskMetaBuilder::new().placement(super::Placement::SameThreadAs(PlacementToken::new(
                self.current,
            ))),
            instantiation,
            future_factory,
        )
    }

    pub fn spawn_multiple_with_meta<FF, F, R>(
        &self,
        meta: impl Into<TaskMeta>,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: TransferFnOnce<F> + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let source = self.current;

        self.dispatcher.spawn_multiple(
            meta.into().placement,
            instantiation,
            async move |_, _, destination| {
                future_factory
                    .transfer(source, destination)
                    .await
                    .call_once()
                    .await
            },
        )
    }
}

impl<TS> Transfer for IsolatedTaskScheduler<TS>
where
    TS: RuntimeThreadState,
{
    async fn transfer(mut self, _source: Domain, destination: Domain) -> Self {
        self.current = destination;
        self
    }
}