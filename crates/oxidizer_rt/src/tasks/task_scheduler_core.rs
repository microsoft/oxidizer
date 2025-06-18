// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use crate::{Dispatch, Instantiation, RemoteJoinHandle, SpawnInstance, TaskMeta};

#[derive(Clone, Debug)]
pub struct TaskSchedulerCore<D> {
    dispatcher: Rc<D>,
}

impl<D> TaskSchedulerCore<D>
where
    D: Dispatch,
{
    pub(crate) const fn new(dispatcher: Rc<D>) -> Self {
        Self { dispatcher }
    }

    pub fn spawn_with_meta<FF, F, R>(
        &self,
        meta: &TaskMeta,
        future_factory: FF,
    ) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(D::ThreadState) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.dispatcher.spawn(meta.placement, future_factory)
    }

    pub fn spawn_multiple_with_meta<FF, F, R>(
        &self,
        instantiation: Instantiation,
        meta: &TaskMeta,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: FnOnce(D::ThreadState, SpawnInstance) -> F + Clone + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.dispatcher
            .spawn_multiple(meta.placement, instantiation, future_factory)
    }
}