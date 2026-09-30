// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::Debug;
use std::sync::Arc;
use std::thread::ThreadId;

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::context::Builtins;
use crate::runtime::dispatch::{DispatcherCore, WorkerIndex};
use crate::runtime::thread::waiter::ThreadWaiter;
use crate::task::join::JoinHandle;

/// Cheap shared access to one runtime's routing and shutdown state.
pub(crate) struct DispatcherClient {
    core: Arc<DispatcherCore<ThreadWaiter>>,
}

impl DispatcherClient {
    pub(in crate::runtime) const fn new(core: Arc<DispatcherCore<ThreadWaiter>>) -> Self {
        Self { core }
    }

    pub(crate) fn worker_index(&self, thread_id: ThreadId) -> Option<WorkerIndex> {
        self.core.worker_index(thread_id)
    }

    pub(crate) fn blocking_worker(&self, worker_index: WorkerIndex) -> Arc<BlockingWorker> {
        self.core.blocking_worker(worker_index)
    }

    pub(crate) fn next_blocking_worker(&self) -> Arc<BlockingWorker> {
        self.core.next_blocking_worker()
    }

    pub(crate) fn owns(&self, thread: &thread_aware::Thread) -> bool {
        self.core.owns(thread)
    }

    pub(crate) fn is_current_blocking_task(&self) -> bool {
        self.core.is_current_blocking_task()
    }

    /// Submits to a previously resolved worker without discarding destination affinity.
    pub(crate) fn spawn_on_worker<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core.spawn_on_worker(worker_index, future_factory)
    }
}

impl Debug for DispatcherClient {
    #[cfg_attr(coverage_nightly, coverage(off))] // Opaque diagnostic formatting only.
    #[cfg_attr(test, mutants::skip)] // Debug formatting not tested
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

impl Clone for DispatcherClient {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
        }
    }
}

impl DispatcherClient {
    pub(crate) fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core.spawn(future_factory)
    }
}

impl DispatcherClient {
    #[cfg_attr(test, mutants::skip)] // Will cause tests to hang due to runtime never stopping.
    pub(crate) fn stop(&self) {
        self.core.stop();
    }

    // Impractical to test real waiting at this API layer. We test the real waiter implementation
    // but not the API layers that simply call the waiter, as it is hard to prove the wait failed.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn wait(&self) {
        self.core.join();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::sync::mpsc;
    use std::task::Waker;

    use observed::Sink;
    use thread_aware::ThreadAware;

    use super::*;
    use crate::runtime::blocking_worker::BlockingPool;
    use crate::runtime::dispatch::{WorkerEndpoint, test_threads};
    use crate::task::scheduler::TaskScheduler;

    #[test]
    fn relocate_updates_task_placement() {
        // Create a dispatcher with 2 workers so we can verify which one receives tasks.
        let (worker0_tx, worker0_rx) = mpsc::channel();
        let (worker1_tx, worker1_rx) = mpsc::channel();
        let wfs = ThreadWaiter::new(vec![]);
        let threads = test_threads(2);
        let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());

        let dispatcher = Arc::new(DispatcherCore::new(
            wfs,
            nonempty::NonEmpty::from_vec(vec![
                WorkerEndpoint {
                    command_tx: worker0_tx,
                    waker: Waker::noop().clone(),
                    thread: threads[0].clone(),
                    blocking_worker: Arc::clone(&blocking_worker),
                },
                WorkerEndpoint {
                    command_tx: worker1_tx,
                    waker: Waker::noop().clone(),
                    thread: threads[1].clone(),
                    blocking_worker: Arc::clone(&blocking_worker),
                },
            ])
            .unwrap(),
            Sink::noop(),
        ));

        let mut scheduler = TaskScheduler::new(DispatcherClient::new(dispatcher), threads[0].clone());

        // Relocate to worker 1.
        scheduler.relocate(Some(&threads[0]), &threads[1]);

        // Spawn a task on worker 1, not worker 0.
        scheduler.spawn(async |_| {});

        // Worker 0 should have received nothing.
        assert!(
            worker0_rx.try_recv().is_err(),
            "task should not be dispatched to original worker after relocate"
        );
        // Worker 1 should have received the task.
        assert!(worker1_rx.try_recv().is_ok(), "task should be dispatched to the relocated worker");
    }

    #[test]
    fn relocate_switches_blocking_worker() {
        let threads = test_threads(2);

        let source_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());
        let destination_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());
        destination_worker.shutdown();

        let dispatcher = Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![]),
            nonempty::NonEmpty::from_vec(vec![
                WorkerEndpoint {
                    command_tx: mpsc::channel().0,
                    waker: Waker::noop().clone(),
                    thread: threads[0].clone(),
                    blocking_worker: Arc::clone(&source_worker),
                },
                WorkerEndpoint {
                    command_tx: mpsc::channel().0,
                    waker: Waker::noop().clone(),
                    thread: threads[1].clone(),
                    blocking_worker: Arc::clone(&destination_worker),
                },
            ])
            .unwrap(),
            Sink::noop(),
        ));

        let mut scheduler = TaskScheduler::new(DispatcherClient::new(dispatcher), threads[0].clone());

        let before = (
            Arc::ptr_eq(scheduler.blocking_worker(), &source_worker),
            Arc::ptr_eq(scheduler.blocking_worker(), &destination_worker),
        );

        scheduler.relocate(Some(&threads[0]), &threads[1]);

        let after = (
            Arc::ptr_eq(scheduler.blocking_worker(), &source_worker),
            Arc::ptr_eq(scheduler.blocking_worker(), &destination_worker),
        );

        assert_eq!((before, after), ((true, false), (false, true)));
    }
}
