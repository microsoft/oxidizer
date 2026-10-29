// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread::ThreadId;

use arty_executor::{ShutdownCause, TaskPlacement};
use foldhash::HashMap;
use nonempty::NonEmpty;
use observed::emit;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::telemetry::events::{RuntimeStopped, RuntimeStopping};
use crate::runtime::thread::waiter::ThreadWaiter;
use crate::runtime::worker::protocol::{AsyncWorkerCommand, QueuedIoAccess, QueuedTaskFactory};
use crate::runtime::{Error, Workers};
use crate::task::Builtins;
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::prepare_remote;

#[derive(Debug)]
pub(crate) struct WorkerEndpoint {
    pub(crate) command_tx: channel::Sender<AsyncWorkerCommand>,
    pub(crate) waker: Waker,
    pub(crate) thread: Thread,
    pub(crate) blocking_worker: Arc<BlockingWorker>,
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
pub(crate) struct DispatcherCore {
    wait_for_shutdown: ThreadWaiter,

    // Completion senders can publish the cause before dispatcher coordination.
    // This separate flag elects the one worker broadcaster.
    shutdown_signaled: AtomicBool,
    stopped_reported: AtomicBool,

    worker_endpoints: NonEmpty<WorkerEndpoint>,
    workers: Workers,

    // Immutable, runtime-local lookup for placement and relocation; ordinary spawn uses a
    // cached index. IDs come only from registered workers, so a non-cryptographic hasher fits.
    worker_indices: HashMap<ThreadId, WorkerIndex>,

    // Minimal effort round-robin scheduling of async tasks.
    next_async_worker_index: AtomicUsize,

    /// Sink for runtime telemetry. Enrichment context is captured from it at spawn time and
    /// restored on every poll of spawned futures.
    sink: observed::Sink,

    remote_results: JoinHandlePool,
}

impl DispatcherCore {
    pub(crate) fn new(
        wait_for_shutdown: ThreadWaiter,
        worker_endpoints: NonEmpty<WorkerEndpoint>,
        sink: observed::Sink,
        shutdown: Shutdown,
    ) -> Arc<Self> {
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
        let workers = Workers::new(
            worker_endpoints
                .iter()
                .map(|endpoint| endpoint.thread.clone())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        assert_eq!(
            worker_indices.len(),
            worker_endpoints.len(),
            "each runtime worker must have a distinct thread ID"
        );
        Arc::new(Self {
            wait_for_shutdown,
            shutdown_signaled: AtomicBool::new(false),
            stopped_reported: AtomicBool::new(false),
            worker_endpoints,
            workers,
            worker_indices,
            next_async_worker_index: AtomicUsize::new(0),
            sink,
            remote_results: JoinHandlePool::new(shutdown),
        })
    }

    #[cfg_attr(test, mutants::skip)] // Tests hang if mutation removes the shutdown signal or worker notifications.
    pub(crate) fn stop_inner(&self, cause: ShutdownCause) -> ShutdownCause {
        self.shutdown().begin(cause);
        let cause = self
            .shutdown()
            .cause()
            .expect("stop_inner begins shutdown before reading the authoritative cause");
        // Shutdown stores the cause independently. This atomic only elects the
        // broadcaster. Observing `true` does not mean notification is complete.
        if self.shutdown_signaled.swap(true, Ordering::AcqRel) {
            return cause;
        }

        emit!(&self.sink, RuntimeStopping);

        for endpoint in &self.worker_endpoints {
            // We ignore the result here because we do not care if the channel is already closed for
            // whatever reason (after all, that is relatively compatible with the "shut down" idea).
            _ = endpoint.command_tx.send(AsyncWorkerCommand::Shutdown);
            endpoint.waker.wake_by_ref();
        }
        cause
    }

    #[expect(clippy::arithmetic_side_effects, reason = "impossible to divide by zero due to NonEmpty")]
    fn next_worker_index(&self) -> WorkerIndex {
        // This counter only chooses a worker; it does not publish any worker state.
        WorkerIndex(self.next_async_worker_index.fetch_add(1, Ordering::Relaxed) % self.worker_endpoints.len())
    }

    pub(crate) fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue(self.next_worker_index(), TaskPlacement::Any, future_factory)
    }

    pub(crate) fn spawn_relocating<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        let worker_index = self.next_worker_index();
        self.spawn_relocating_on_worker(worker_index, TaskPlacement::Any, future_factory)
    }

    pub(crate) fn spawn_at_worker_relocating<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        self.spawn_relocating_on_worker(worker_index, TaskPlacement::SpecificThread, future_factory)
    }

    pub(crate) fn worker_index(&self, thread_id: ThreadId) -> Option<WorkerIndex> {
        self.worker_indices.get(&thread_id).copied()
    }

    pub(crate) const fn workers(&self) -> &Workers {
        &self.workers
    }

    pub(crate) fn owns(&self, thread: &Thread) -> bool {
        self.worker_endpoints.first().thread.owner() == thread.owner()
    }

    pub(crate) fn access_io(&self, worker_index: WorkerIndex, access: Box<dyn FnOnce(&Builtins) + Send>) -> Result<(), Error> {
        if matches!(self.shutdown().state(), ShutdownState::Started) {
            return Err(self.runtime_shutting_down_error());
        }
        let endpoint = self
            .worker_endpoints
            .get(usize::from(worker_index))
            .expect("worker index must identify a registered runtime worker");
        let send_result = endpoint.command_tx.send(AsyncWorkerCommand::AccessIo {
            access: QueuedIoAccess::new(access),
        });
        endpoint.waker.wake_by_ref();
        send_result.map_err(|_error| self.runtime_shutting_down_error())
    }

    fn runtime_shutting_down_error(&self) -> Error {
        Error::from_shutdown(self.shutdown()).unwrap_or_else(Error::runtime_shutting_down)
    }

    pub(crate) const fn shutdown(&self) -> &Shutdown {
        self.remote_results.shutdown()
    }

    #[cfg_attr(test, mutants::skip)] // Removing this wait guard deadlocks runtime-owner teardown.
    pub(crate) fn is_current_blocking_thread(&self) -> bool {
        self.worker_endpoints
            .iter()
            .any(|endpoint| endpoint.blocking_worker.owns_current_thread())
    }

    pub(crate) fn next_blocking_worker(&self) -> Arc<BlockingWorker> {
        self.blocking_worker(self.next_worker_index())
    }

    pub(crate) fn blocking_worker(&self, worker_index: WorkerIndex) -> Arc<BlockingWorker> {
        Arc::clone(
            &self
                .worker_endpoints
                .get(usize::from(worker_index))
                .expect("scheduler worker index must be registered")
                .blocking_worker,
        )
    }

    pub(crate) fn wake_worker(&self, worker_index: WorkerIndex) {
        self.worker_endpoints
            .get(usize::from(worker_index))
            .expect("worker index must identify a registered runtime worker")
            .waker
            .wake_by_ref();
    }

    pub(crate) fn spawn_on_worker<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue(worker_index, TaskPlacement::SameThread, future_factory)
    }

    /// Records the execution worker so a successful result can relocate when its join is polled.
    fn spawn_relocating_on_worker<FF, F, R>(&self, worker_index: WorkerIndex, placement: TaskPlacement, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        let source = self.worker_endpoints[usize::from(worker_index)].thread.clone();
        self.enqueue(worker_index, placement, future_factory).relocate_result_from(source)
    }

    fn enqueue<FF, F, R>(&self, worker_index: WorkerIndex, placement: TaskPlacement, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        if let Some(join_handle) = JoinHandle::try_from_shutdown(self.shutdown()) {
            drop(future_factory);
            return join_handle;
        }
        let endpoint = self
            .worker_endpoints
            .get(usize::from(worker_index))
            .expect("worker index must identify a registered runtime worker");
        // Capture enrichment context on the calling thread before sending to the worker.
        let parent_task_enrichment = self.sink.transfer_context();
        let (future_factory, join_handle) = prepare_remote(&self.remote_results, future_factory, parent_task_enrichment, placement);

        // There is nothing we can really do if the worker is already gone and closed the channel.
        // That may be the case when we landed here when the runtime was already shutting down.
        _ = endpoint.command_tx.send(AsyncWorkerCommand::EnqueueTask {
            future_factory: QueuedTaskFactory::new(future_factory),
        });

        endpoint.waker.wake_by_ref();

        join_handle
    }

    /// Waits for the runtime to shut down and all worker threads to exit.
    ///
    /// Safe to call multiple times.
    pub(crate) fn join(&self) -> Result<(), Error> {
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
    use std::error::Error as _;
    use std::pin::pin;
    use std::task;
    use std::task::Poll;

    use arty_executor::ShutdownError;
    use testing_aids::TEST_TIMEOUT;

    use super::*;
    use crate::runtime::blocking_worker::{BlockingPanicPolicy, BlockingPool};
    use crate::runtime::dispatch::{DispatcherClient, test_threads};

    #[cfg_attr(test, mutants::skip)]
    fn endpoint(tx: channel::Sender<AsyncWorkerCommand>, thread: &Thread) -> WorkerEndpoint {
        WorkerEndpoint {
            command_tx: tx,
            waker: Waker::noop().clone(),
            thread: thread.clone(),
            blocking_worker: BlockingWorker::new(
                BlockingPool::new(None),
                observed::Sink::noop(),
                Shutdown::default(),
                BlockingPanicPolicy::Ignore,
            ),
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
            ThreadWaiter::new(Vec::new()),
            NonEmpty::from_vec(vec![endpoint(first_tx, &first), endpoint(second_tx, &second)]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
        );
    }

    #[test]
    fn dispatch_spawn_single() {
        // We dispatch two tasks to the only worker.

        let (worker_tx, worker_rx) = channel::unbounded();
        let wfs = ThreadWaiter::new(Vec::new());
        let threads = test_threads(1);

        let dispatcher = DispatcherCore::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker_tx, &threads[0])]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
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
        let wfs = ThreadWaiter::new(Vec::new());
        let threads = test_threads(2);

        let dispatcher = DispatcherCore::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker1_tx, &threads[0]), endpoint(worker2_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
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
        let wfs = ThreadWaiter::new(Vec::new());
        let threads = test_threads(2);

        let dispatcher = DispatcherCore::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker1_tx, &threads[0]), endpoint(worker2_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
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
            ThreadWaiter::new(Vec::new()),
            NonEmpty::from_vec(vec![endpoint(first_tx, &threads[0]), endpoint(second_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
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
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint(tx, &threads[0])),
            observed::Sink::noop(),
            Shutdown::default(),
        );

        assert!(dispatcher.worker_index(threads[1].id()).is_none());
    }

    #[test]
    fn shutdown_state_is_shared_with_dispatcher_client() {
        let (tx, _rx) = channel::unbounded();
        let threads = test_threads(1);
        let core = DispatcherCore::new(
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint(tx, &threads[0])),
            observed::Sink::noop(),
            Shutdown::default(),
        );
        let client = DispatcherClient::new(Arc::clone(&core));
        let shutdown = client.shutdown();

        core.stop_inner(ShutdownCause::default());

        assert_eq!(shutdown.state(), ShutdownState::Started);
    }

    #[test]
    fn io_access_after_shutdown_is_rejected() {
        let (tx, rx) = channel::unbounded();
        let threads = test_threads(1);
        let dispatcher = DispatcherCore::new(
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint(tx, &threads[0])),
            observed::Sink::noop(),
            Shutdown::default(),
        );
        dispatcher.stop_inner(ShutdownCause::default());
        let invoked = Arc::new(AtomicBool::new(false));

        assert!(
            dispatcher
                .access_io(
                    WorkerIndex(0),
                    Box::new({
                        let invoked = Arc::clone(&invoked);
                        move |_| invoked.store(true, Ordering::Relaxed)
                    }),
                )
                .is_err()
        );

        // Rejection is synchronous and the callback publishes no other data.
        assert!(!invoked.load(Ordering::Relaxed));
        assert!(matches!(drain_rx(&rx).as_slice(), [AsyncWorkerCommand::Shutdown]));
    }

    #[test]
    fn worker_failure_is_attached_to_rejected_task_joins() {
        let (tx, _rx) = channel::unbounded();
        let threads = test_threads(1);
        let dispatcher = DispatcherCore::new(
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint(tx, &threads[0])),
            observed::Sink::noop(),
            Shutdown::default(),
        );
        dispatcher.stop_inner(ShutdownError::new(Error::worker_panicked()).into());

        let mut join = pin!(dispatcher.spawn(|_| async {}));
        let Poll::Ready(Err(error)) = join.as_mut().poll(&mut task::Context::from_waker(Waker::noop())) else {
            panic!("shutdown rejection must be immediately ready");
        };
        let shutdown = error
            .source()
            .unwrap()
            .downcast_ref::<ShutdownError>()
            .expect("caused shutdown exposes ShutdownError");

        assert_eq!(shutdown.to_string(), "a runtime worker panicked");
    }

    #[test]
    fn dispatcher_stop_sends_one_stop_command_to_each_worker() {
        let (worker1_tx, worker1_rx) = channel::unbounded();
        let (worker2_tx, worker2_rx) = channel::unbounded();
        let wfs = ThreadWaiter::new(Vec::new());
        let threads = test_threads(2);

        let dispatcher = DispatcherCore::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker1_tx, &threads[0]), endpoint(worker2_tx, &threads[1])]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
        );

        assert_eq!(drain_rx(&worker1_rx).len(), 0);
        assert_eq!(drain_rx(&worker2_rx).len(), 0);

        dispatcher.stop_inner(ShutdownCause::default());

        assert_eq!(drain_rx(&worker1_rx).len(), 1);
        assert_eq!(drain_rx(&worker2_rx).len(), 1);

        // Call for a stop twice - we expect no further stop commands to be sent.
        dispatcher.stop_inner(ShutdownCause::default());

        assert_eq!(drain_rx(&worker1_rx).len(), 0);
        assert_eq!(drain_rx(&worker2_rx).len(), 0);
    }

    #[test]
    fn spawn_after_command_channel_closed_returns_disconnected_join_handle() {
        // Once the worker has closed the command channel (e.g. because it is shutting down)
        // we ignore any further spawn requests (they simply return disconnected join handles).

        let (worker_tx, worker_rx) = channel::unbounded();
        let wfs = ThreadWaiter::new(Vec::new());
        let threads = test_threads(1);

        let dispatcher = DispatcherCore::new(
            wfs,
            NonEmpty::from_vec(vec![endpoint(worker_tx, &threads[0])]).unwrap(),
            observed::Sink::noop(),
            Shutdown::default(),
        );

        dispatcher.stop_inner(ShutdownCause::default());

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

    fn shutdown_dispatcher() -> (Arc<DispatcherCore>, DispatcherClient, Shutdown) {
        let (tx, _rx) = channel::unbounded();
        let thread = test_threads(1).remove(0);
        let shutdown = Shutdown::default();
        let core = DispatcherCore::new(
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint(tx, &thread)),
            observed::Sink::noop(),
            shutdown.clone(),
        );
        let client = DispatcherClient::new(Arc::clone(&core));
        (core, client, shutdown)
    }

    #[test]
    fn published_shutdown_cause_is_shared_with_dispatcher_and_client() {
        let (core, client, shutdown) = shutdown_dispatcher();
        let cause = ShutdownCause::panic();
        let expected = cause.as_error().unwrap().clone();

        shutdown.begin(cause);

        for state in [core.shutdown(), client.shutdown(), &shutdown] {
            assert_eq!(state.state(), ShutdownState::Started);
            assert!(std::ptr::addr_eq(state.error().unwrap().inner(), expected.inner()));
        }
    }

    #[test]
    fn direct_shutdown_publication_starts_shutdown() {
        let (core, _client, shutdown) = shutdown_dispatcher();

        core.shutdown().begin(ShutdownCause::default());

        assert_eq!(shutdown.state(), ShutdownState::Started);
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
