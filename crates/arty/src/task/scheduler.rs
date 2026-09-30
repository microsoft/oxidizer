// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use thread_aware::{Thread, ThreadAware};

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::context::Builtins;
use crate::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::task::join::JoinHandle;

/// Submits asynchronous and blocking tasks to an Arty runtime.
///
/// [`Runtime::task_scheduler`](crate::runtime::Runtime::task_scheduler) returns a detached
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
/// Shutdown cancels pending tasks. Their join handles return
/// [`JoinError`](crate::task::JoinError) with `is_shutdown() == true`.
/// New submissions are rejected immediately without invoking their factories.
/// See the [documentation guides](crate#documentation) for factory,
/// future, and result examples.
#[derive(Debug, Clone)]
pub struct TaskScheduler {
    dispatcher: DispatcherClient,
    binding: Option<Binding>,
}

#[derive(Debug, Clone)]
struct Binding {
    thread: Thread,
    worker_index: WorkerIndex,
    blocking_worker: Arc<BlockingWorker>,
}

impl TaskScheduler {
    pub(crate) fn new(dispatcher: DispatcherClient, current: Thread) -> Self {
        let worker_index = dispatcher
            .worker_index(current.id())
            .expect("scheduler thread must be registered with its dispatcher");
        let blocking_worker = dispatcher.blocking_worker(worker_index);
        Self {
            dispatcher,
            binding: Some(Binding {
                thread: current,
                worker_index,
                blocking_worker,
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
            blocking_worker: self.dispatcher.blocking_worker(worker_index),
        });
    }

    /// Starts a task on the associated worker, or round-robin when detached.
    ///
    /// The factory receives owned [`Builtins`] on the worker before its future is
    /// constructed. The future itself need not be [`Send`]. Ordinary captures and
    /// results are not automatically relocated; use [`spawn_anywhere`](Self::spawn_anywhere)
    /// for explicit payload relocation.
    ///
    /// Tasks must not block their asynchronous worker. Use
    /// [`spawn_blocking`](Self::spawn_blocking) for synchronous blocking calls.
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

    /// Starts blocking work without blocking an asynchronous worker.
    ///
    /// Bound schedulers use their worker's blocking pool; detached schedulers select a
    /// worker's pool round-robin. Shutdown rejects new work and cancels queued
    /// callbacks before invocation. An already-running blocking closure cannot
    /// be forcibly interrupted and is allowed to finish.
    ///
    /// Blocking work uses a separate thread pool so asynchronous workers remain
    /// responsive. Pool sizing targets blocking calls rather than sustained CPU work.
    pub fn spawn_blocking<B, R>(&self, body: B) -> JoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if self.dispatcher.is_shutting_down() {
            return JoinHandle::shutdown();
        }
        match &self.binding {
            Some(binding) => binding.blocking_worker.spawn_blocking(body),
            None => self.dispatcher.next_blocking_worker().spawn_blocking(body),
        }
    }

    #[cfg(test)]
    pub(crate) fn blocking_worker(&self) -> &Arc<BlockingWorker> {
        &self.binding.as_ref().unwrap().blocking_worker
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
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn assert_send_sync() {
        static_assertions::assert_impl_all!(TaskScheduler: Send, Sync);
    }

    #[cfg(not(miri))]
    #[test]
    fn a_foreign_owner_cannot_rebind_a_registered_thread_id() {
        struct RelocationSource(Option<Thread>);

        impl ThreadAware for RelocationSource {
            fn relocate(&mut self, source: Option<&Thread>, _: &Thread) {
                self.0 = source.cloned();
            }
        }

        let runtime = crate::runtime::Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(std::num::NonZeroUsize::MIN))
            .build()
            .unwrap();
        let (source, mut scheduler) = runtime
            .task_scheduler()
            .spawn(async |cx| (cx.thread().clone(), cx.scheduler().clone()))
            .wait()
            .unwrap();
        let foreign = thread_aware::ThreadBuilder::default().build(source.id());
        assert_ne!(source.owner(), foreign.owner());

        scheduler.relocate(None, &foreign);
        let scheduler: &TaskScheduler = scheduler.as_ref();
        let actual = scheduler
            .spawn_anywhere(RelocationSource(None), |probe| async move { probe.0 })
            .wait()
            .unwrap();
        assert_eq!(actual, Some(source));
    }
}
