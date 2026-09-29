// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use thread_aware::{Thread, ThreadAware};

use crate::rt::runtime::context::Builtins;
use crate::rt::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::rt::runtime::system_worker::SystemWorker;
use crate::rt::task::join::JoinHandle;

/// Submits asynchronous and blocking system tasks to an Arty runtime.
///
/// [`Runtime::task_scheduler`](crate::rt::Runtime::task_scheduler) returns a detached
/// scheduler. Its submissions select workers round-robin, sharing the selection
/// sequence with other detached handles of the same runtime.
///
/// A scheduler obtained from [`Builtins::scheduler`] is bound to that worker.
/// Ordinary spawning preserves its affinity. Use [`spawn_anywhere`](Self::spawn_anywhere)
/// to distribute an independent unit of work and relocate its explicit payload.
///
/// Cloning does not change the binding or keep the runtime running.
/// [`ThreadAware`] relocation binds this handle to a registered worker of its original
/// runtime. Foreign or unregistered destinations leave the binding unchanged.
///
/// Accepted tasks may be discarded during shutdown. Their join handles then remain
/// pending. Submitting to an already closed worker also returns a pending join handle.
#[derive(Debug, Clone)]
pub struct TaskScheduler {
    dispatcher: DispatcherClient,
    binding: Option<Binding>,
}

#[derive(Debug, Clone)]
struct Binding {
    thread: Thread,
    worker_index: WorkerIndex,
    system_worker: Arc<SystemWorker>,
}

impl TaskScheduler {
    pub(crate) fn new(dispatcher: DispatcherClient, current: Thread) -> Self {
        let worker_index = dispatcher
            .worker_index(current.id())
            .expect("scheduler thread must be registered with its dispatcher");
        let system_worker = dispatcher.system_worker(worker_index);
        Self {
            dispatcher,
            binding: Some(Binding {
                thread: current,
                worker_index,
                system_worker,
            }),
        }
    }

    pub(crate) const fn detached(dispatcher: DispatcherClient) -> Self {
        Self { dispatcher, binding: None }
    }

    pub(crate) fn current_worker_index(&self) -> WorkerIndex {
        self.binding
            .as_ref()
            .expect("worker initialization uses a bound scheduler")
            .worker_index
    }

    pub(crate) fn resolve_worker_index(&self, thread: &Thread) -> Option<WorkerIndex> {
        self.dispatcher
            .owns(thread)
            .then(|| self.dispatcher.worker_index(thread.id()))
            .flatten()
    }

    pub(crate) fn relocate_to_worker(&mut self, destination: &Thread, worker_index: WorkerIndex) {
        self.binding = Some(Binding {
            thread: destination.clone(),
            worker_index,
            system_worker: self.dispatcher.system_worker(worker_index),
        });
    }

    /// Starts a task on the associated worker, or round-robin when detached.
    ///
    /// The factory is sent to the worker before its future is constructed. The future
    /// itself need not be [`Send`]. Ordinary captures and results are not automatically
    /// relocated; use [`spawn_anywhere`](Self::spawn_anywhere) for explicit payload relocation.
    #[doc = include_str!("../../../docs/snippets/async_task.md")]
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        match &self.binding {
            Some(binding) => self.dispatcher.spawn_on_worker(binding.worker_index, future_factory),
            None => self.dispatcher.spawn(future_factory),
        }
    }

    /// Distributes a task round-robin and relocates its payload on the destination worker.
    ///
    /// The factory is a function pointer so thread-affine captures must be supplied
    /// explicitly as `data`. The source coordinate is the binding of this scheduler, or
    /// unknown for a detached scheduler. The completed result is not relocated.
    pub fn spawn_anywhere<D, F, R>(&self, data: D, f: fn(D) -> F) -> JoinHandle<R>
    where
        D: ThreadAware + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let source = self.binding.as_ref().map(|binding| binding.thread.clone());
        self.dispatcher.spawn(async move |cx| {
            let mut data = data;
            data.relocate(source.as_ref(), cx.thread());
            f(data).await
        })
    }

    /// Starts blocking system work without blocking an asynchronous worker.
    ///
    /// Bound schedulers use their worker's system pool; detached schedulers select a
    /// worker's pool round-robin. Shutdown prevents new system work from starting,
    /// but waits for previously accepted system work to finish.
    #[doc = include_str!("../../../docs/snippets/system_task.md")]
    pub fn spawn_system<B, R>(&self, body: B) -> JoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        match &self.binding {
            Some(binding) => binding.system_worker.spawn_system(body),
            None => self.dispatcher.next_system_worker().spawn_system(body),
        }
    }

    #[cfg(test)]
    pub(crate) fn system_worker(&self) -> &Arc<SystemWorker> {
        &self.binding.as_ref().unwrap().system_worker
    }
}

impl ThreadAware for TaskScheduler {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.binding.as_ref().is_some_and(|binding| binding.thread == *destination) {
            return;
        }
        if let Some(index) = self.resolve_worker_index(destination) {
            self.relocate_to_worker(destination, index);
        }
    }
}

impl AsRef<Self> for TaskScheduler {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_send_sync() {
        static_assertions::assert_impl_all!(TaskScheduler: Send, Sync);
    }
}
