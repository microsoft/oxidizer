// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread::ThreadId;

use foldhash::HashMap;
use nonempty::NonEmpty;
use observed::emit;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::Thread;

use crate::runtime::Error;
use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::telemetry::events::{PlacementLabel, RuntimeStopped, RuntimeStopping, TaskSpawned};
use crate::runtime::thread::waiter::WaitForShutdown;
use crate::runtime::worker::protocol::AsyncWorkerCommand;
use crate::task::Builtins;
use crate::task::execution::prepare_remote;
use crate::task::join::JoinHandle;

#[derive(Debug)]
pub(in crate::runtime) struct WorkerEndpoint {
    pub(in crate::runtime) command_tx: channel::Sender<AsyncWorkerCommand>,
    pub(in crate::runtime) waker: Waker,
    pub(in crate::runtime) thread: Thread,
    pub(in crate::runtime) blocking_worker: Arc<BlockingWorker>,
}

/// Index resolved against a dispatcher's registered workers, not a hardware processor ID.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkerIndex(usize);

impl From<WorkerIndex> for usize {
    fn from(value: WorkerIndex) -> Self {
        value.0
    }
}

/// Directs commands to specific workers.
///
/// The dispatcher is aware of all workers that exist, their association with different hardware
/// resources, and what sort of commands are appropriate for which workers. It sends commands to
/// the relevant workers when commanded to by the caller.
///
/// This is used by:
/// 1. `Runtime` to send commands to workers from arbitrary code.
/// 2. Task-specific context objects (e.g. `TaskContext`) for the same purpose.
///
/// Commands are received as function calls to the dispatcher and delivered via message channels.
#[derive(Debug)]
pub(in crate::runtime) struct DispatcherCore<WFS> {
    wait_for_shutdown: WFS,

    /// We record whether shutdown has started, both to avoid double-shutdown and to execute
    /// special-case logic in some situations that need special handling during shutdown.
    shutdown_started: Arc<AtomicBool>,
    stopped_reported: AtomicBool,

    worker_endpoints: NonEmpty<WorkerEndpoint>,

    // Immutable, runtime-local lookup for placement and relocation; ordinary spawn uses a
    // cached index. IDs come only from registered workers, so a non-cryptographic hasher fits.
    worker_indices: HashMap<ThreadId, WorkerIndex>,

    // Minimal effort round-robin scheduling of async tasks.
    next_async_worker_index: AtomicUsize,

    /// Sink for runtime telemetry. Enrichment context is captured from it at spawn time and
    /// restored on every poll of spawned futures.
    sink: observed::Sink,
}

impl<WFS> DispatcherCore<WFS> {
    pub(in crate::runtime) fn new(wait_for_shutdown: WFS, worker_endpoints: NonEmpty<WorkerEndpoint>, sink: observed::Sink) -> Self {
        let owner = worker_endpoints.first().thread.owner();
        assert!(
            worker_endpoints.iter().all(|endpoint| endpoint.thread.owner() == owner),
            "each runtime worker must have the same runtime owner"
        );
        let worker_indices: HashMap<_, _> = worker_endpoints
            .iter()
            .enumerate()
            .map(|(index, endpoint)| (endpoint.thread.id(), WorkerIndex(index)))
            .collect();
        assert_eq!(
            worker_indices.len(),
            worker_endpoints.len(),
            "each runtime worker must have a distinct thread ID"
        );
        Self {
            wait_for_shutdown,
            shutdown_started: Arc::new(AtomicBool::new(false)),
            stopped_reported: AtomicBool::new(false),
            worker_endpoints,
            worker_indices,
            next_async_worker_index: AtomicUsize::new(0),
            sink,
        }
    }

    /// Stops the runtime. Safe to call multiple times.
    #[cfg_attr(test, mutants::skip)] // Tests will hang if we mutate away the stop signal.
    pub(in crate::runtime) fn stop(&self) {
        // If we've already started shutting down, don't do it again.
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }

        emit!(&self.sink, RuntimeStopping);

        for endpoint in &self.worker_endpoints {
            endpoint.blocking_worker.shutdown();
            // We ignore the result here because we do not care if the channel is already closed for
            // whatever reason (after all, that is relatively compatible with the "shut down" idea).
            _ = endpoint.command_tx.send(AsyncWorkerCommand::Shutdown);
            endpoint.waker.wake_by_ref();
        }
    }

    #[expect(clippy::arithmetic_side_effects, reason = "impossible to divide by zero due to NonEmpty")]
    fn next_worker_index(&self) -> WorkerIndex {
        // This counter only chooses a worker; it does not publish any worker state.
        WorkerIndex(self.next_async_worker_index.fetch_add(1, Ordering::Relaxed) % self.worker_endpoints.len())
    }

    pub(in crate::runtime) fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue(self.next_worker_index(), "any", future_factory)
    }

    pub(in crate::runtime) fn worker_index(&self, thread_id: ThreadId) -> Option<WorkerIndex> {
        self.worker_indices.get(&thread_id).copied()
    }

    pub(in crate::runtime) fn spawn_everywhere<M, FF, F, R>(&self, mut make_factory: M) -> Vec<JoinHandle<R>>
    where
        M: FnMut() -> FF,
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        (0..self.worker_endpoints.len())
            .map(|index| self.enqueue(WorkerIndex(index), "any", make_factory()))
            .collect()
    }

    pub(in crate::runtime) fn owns(&self, thread: &Thread) -> bool {
        self.worker_endpoints.first().thread.owner() == thread.owner()
    }

    pub(in crate::runtime) fn shutdown_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_started)
    }

    pub(in crate::runtime) fn is_shutting_down(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire)
    }

    pub(in crate::runtime) fn is_current_blocking_task(&self) -> bool {
        self.worker_endpoints
            .iter()
            .any(|endpoint| endpoint.blocking_worker.is_current_task())
    }

    pub(in crate::runtime) fn next_blocking_worker(&self) -> Arc<BlockingWorker> {
        self.blocking_worker(self.next_worker_index())
    }

    pub(in crate::runtime) fn blocking_worker(&self, worker_index: WorkerIndex) -> Arc<BlockingWorker> {
        Arc::clone(
            &self
                .worker_endpoints
                .get(usize::from(worker_index))
                .expect("scheduler worker index must be registered")
                .blocking_worker,
        )
    }

    pub(in crate::runtime) fn spawn_on_worker<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue(worker_index, "same_thread", future_factory)
    }

    fn enqueue<FF, F, R>(&self, worker_index: WorkerIndex, placement: &'static str, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        if self.is_shutting_down() {
            return JoinHandle::shutdown();
        }
        let endpoint = self
            .worker_endpoints
            .get(usize::from(worker_index))
            .expect("worker index must identify a registered runtime worker");
        // Capture enrichment context on the calling thread before sending to the worker.
        let parent_task_enrichment = self.sink.transfer_context();
        let (future_factory, join_handle) = prepare_remote(future_factory, parent_task_enrichment, self.sink.clone());

        // There is nothing we can really do if the worker is already gone and closed the channel.
        // That may be the case when we landed here when the runtime was already shutting down.
        let send_result = endpoint.command_tx.send(AsyncWorkerCommand::EnqueueTask {
            future_factory: Some(future_factory),
        });

        if send_result.is_ok() {
            emit!(
                &self.sink,
                TaskSpawned {
                    placement: PlacementLabel(placement),
                }
            );
        }
        endpoint.waker.wake_by_ref();

        join_handle
    }
}

impl<WFS> DispatcherCore<WFS>
where
    WFS: WaitForShutdown,
{
    /// Waits for the runtime to shut down and all worker threads to exit.
    ///
    /// Safe to call multiple times.
    pub(in crate::runtime) fn join(&self) -> Result<(), Error> {
        let outcome = self.wait_for_shutdown.wait();
        if !self.stopped_reported.swap(true, Ordering::Relaxed) {
            emit!(&self.sink, RuntimeStopped);
        }
        outcome
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::pin::pin;
    use std::task;
    use std::task::Poll;

    use testing_aids::TEST_TIMEOUT;

    use super::*;
    use crate::runtime::blocking_worker::BlockingPool;
    use crate::runtime::dispatch::test_threads;
    use crate::runtime::thread::waiter::MockWaitForShutdown;

    #[cfg_attr(test, mutants::skip)]
    fn endpoint(tx: channel::Sender<AsyncWorkerCommand>, thread: &Thread) -> WorkerEndpoint {
        WorkerEndpoint {
            command_tx: tx,
            waker: Waker::noop().clone(),
            thread: thread.clone(),
            blocking_worker: BlockingWorker::new(BlockingPool::new(None), observed::Sink::noop()),
        }
    }

    #[test]
    #[should_panic(expected = "same runtime owner")]
    fn dispatcher_rejects_mixed_runtime_owners() {
        let first = test_threads(1).remove(0);
        let second = test_threads(1).remove(0);
        let (first_tx, _first_rx) = channel::unbounded();
        let (second_tx, _second_rx) = channel::unbounded();

        DispatcherCore::new(
            MockWaitForShutdown::new(),
            NonEmpty::from_vec(vec![endpoint(first_tx, &first), endpoint(second_tx, &second)]).unwrap(),
            observed::Sink::noop(),
        );
    }

    #[test]
    fn dispatch_spawn_single() {
        // We dispatch two tasks to the only worker.

        let (worker_tx, worker_rx) = channel::unbounded();
        let wfs = MockWaitForShutdown::new();
        let threads = test_threads(1);

        let dispatcher = DispatcherCore::<_>::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker_tx, &threads[0])]).unwrap(),
            observed::Sink::noop(),
        );

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        assert_eq!(drain_rx(&worker_rx).len(), 2);
    }

    #[test]
    fn dispatch_spawn_two() {
        // We dispatch two tasks to two workers, one each. Note that this test assumes we have
        // "fair" scheduling where tasks are evenly spread (e.g. round-robin). This is a coincidence
        // today as it is simply an implementation detail. If we change our scheduling logic, we
        // have to refactor this test to plug in a specific round-robin scheduling strategy to
        // accommodate the expectations (or perhaps move such a test to a test of the strategy).

        let (worker1_tx, worker1_rx) = channel::unbounded();
        let (worker2_tx, worker2_rx) = channel::unbounded();
        let wfs = MockWaitForShutdown::new();
        let threads = test_threads(2);

        let dispatcher = DispatcherCore::<_>::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker1_tx, &threads[0]), endpoint(worker2_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
        );

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        assert_eq!(drain_rx(&worker1_rx).len(), 1);
        assert_eq!(drain_rx(&worker2_rx).len(), 1);
    }

    #[test]
    fn round_robin_repeats_without_resetting() {
        let (worker1_tx, worker1_rx) = channel::unbounded();
        let (worker2_tx, worker2_rx) = channel::unbounded();
        let wfs = MockWaitForShutdown::new();
        let threads = test_threads(2);

        let dispatcher = DispatcherCore::<_>::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker1_tx, &threads[0]), endpoint(worker2_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
        );

        for _ in 0..4 {
            dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });
        }

        assert_eq!(drain_rx(&worker1_rx).len(), 2);
        assert_eq!(drain_rx(&worker2_rx).len(), 2);
    }

    #[test]
    fn same_thread_placement_selects_matching_worker() {
        let threads = test_threads(2);
        let (first_tx, first_rx) = channel::unbounded();
        let (second_tx, second_rx) = channel::unbounded();
        let dispatcher = DispatcherCore::new(
            MockWaitForShutdown::new(),
            NonEmpty::from_vec(vec![endpoint(first_tx, &threads[0]), endpoint(second_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
        );

        let index = dispatcher.worker_index(threads[1].id()).unwrap();
        dispatcher.spawn_on_worker(index, |_| async {});

        assert_eq!((drain_rx(&first_rx).len(), drain_rx(&second_rx).len()), (0, 1),);
    }

    #[test]
    fn unregistered_thread_has_no_worker_index() {
        let threads = test_threads(2);
        let (tx, _rx) = channel::unbounded();
        let dispatcher = DispatcherCore::new(
            MockWaitForShutdown::new(),
            NonEmpty::new(endpoint(tx, &threads[0])),
            observed::Sink::noop(),
        );

        assert!(dispatcher.worker_index(threads[1].id()).is_none());
    }

    #[test]
    fn dispatcher_stop_sends_one_stop_command_to_each_worker() {
        let (worker1_tx, worker1_rx) = channel::unbounded();
        let (worker2_tx, worker2_rx) = channel::unbounded();
        let wfs = MockWaitForShutdown::new();
        let threads = test_threads(2);

        let dispatcher = DispatcherCore::<_>::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker1_tx, &threads[0]), endpoint(worker2_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
        );

        assert_eq!(drain_rx(&worker1_rx).len(), 0);
        assert_eq!(drain_rx(&worker2_rx).len(), 0);

        dispatcher.stop();

        assert_eq!(drain_rx(&worker1_rx).len(), 1);
        assert_eq!(drain_rx(&worker2_rx).len(), 1);

        // Call for a stop twice - we expect no further stop commands to be sent.
        dispatcher.stop();

        assert_eq!(drain_rx(&worker1_rx).len(), 0);
        assert_eq!(drain_rx(&worker2_rx).len(), 0);
    }

    #[test]
    fn spawn_after_command_channel_closed_returns_disconnected_join_handle() {
        // Once the worker has closed the command channel (e.g. because it is shutting down)
        // we ignore any further spawn requests (they simply return disconnected join handles).

        let (worker_tx, worker_rx) = channel::unbounded();
        let wfs = MockWaitForShutdown::new();
        let threads = test_threads(1);

        let dispatcher = DispatcherCore::<_>::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker_tx, &threads[0])]).unwrap(),
            observed::Sink::noop(),
        );

        dispatcher.stop();

        // Drain the shutdown command.
        _ = worker_rx.recv_timeout(TEST_TIMEOUT).unwrap();

        // Close the command channel.
        drop(worker_rx);

        let join_handle = dispatcher.spawn(|_| async move {});
        let join_handle = pin!(join_handle);

        let mut cx = task::Context::from_waker(Waker::noop());
        let Poll::Ready(Err(error)) = join_handle.poll(&mut cx) else {
            panic!("a disconnected join must report shutdown");
        };
        assert!(error.is_shutdown());
    }

    #[cfg_attr(test, mutants::skip)]
    fn drain_rx(worker_rx: &channel::Receiver<AsyncWorkerCommand>) -> Vec<AsyncWorkerCommand> {
        let mut commands = Vec::new();

        loop {
            match worker_rx.try_recv() {
                Ok(command) => commands.push(command),
                Err(error) => {
                    assert!(error.is_empty(), "worker channel disconnected");
                    return commands;
                }
            }
        }
    }
}
