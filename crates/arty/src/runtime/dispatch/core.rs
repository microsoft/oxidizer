// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared worker routing and shutdown state.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread::ThreadId;

use arty_executor::{ShutdownCause, TaskPlacement};
use foldhash::HashMap;
use nonempty::NonEmpty;
use performables::arc::Arc;
use performables::sync::mutex::Mutex;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::hook::{HookRegistration, Spawn};
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::worker::asynchronous::{CommandSender, LocalWorker, QueuedFactory, QueuedIoAccess, SpawnCommand, SystemCommand};
use crate::runtime::worker::blocking::Worker;
use crate::runtime::{Error, Workers};
use crate::task::Context;
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::{RemoteFutureFactory, prepare_remote};

const ERR_POISONED_HOOKS: &str = "runtime hook lock is poisoned by a prior panic";

#[derive(Clone, Debug)]
pub(crate) struct WorkerEndpoint {
    pub(crate) commands: CommandSender,
    pub(crate) waker: Waker,
    pub(crate) thread: Thread,
    pub(crate) blocking_worker: Arc<Worker>,
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
    // Completion senders can publish the cause before dispatcher coordination.
    // This separate flag elects the one worker broadcaster.
    shutdown_signaled: AtomicBool,

    worker_endpoints: NonEmpty<WorkerEndpoint>,
    workers: Workers,

    // Immutable, runtime-local lookup for placement and relocation; ordinary spawn uses a
    // cached index. IDs come only from registered workers, so a non-cryptographic hasher fits.
    worker_indices: HashMap<ThreadId, WorkerIndex>,

    retained_hooks: Mutex<Vec<HookRegistration>>,

    // Minimal effort round-robin scheduling of async tasks.
    next_async_worker_index: AtomicUsize,

    remote_results: JoinHandlePool,
}

impl DispatcherCore {
    pub(crate) fn new(
        worker_endpoints: NonEmpty<WorkerEndpoint>,
        shutdown: Shutdown,
        hook_registrations: Vec<HookRegistration>,
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
            shutdown_signaled: AtomicBool::new(false),
            worker_endpoints,
            workers,
            worker_indices,
            retained_hooks: Mutex::new(hook_registrations.into_iter().filter(HookRegistration::is_retained).collect()),
            next_async_worker_index: AtomicUsize::new(0),
            remote_results: JoinHandlePool::new(shutdown),
        })
    }

    pub(crate) fn stop_inner(&self, cause: ShutdownCause) -> ShutdownCause {
        self.shutdown().begin(cause);
        let cause = self.shutdown().cause().unwrap_or_default();
        // Shutdown stores the cause independently. This atomic only elects the
        // broadcaster. Observing `true` does not mean notification is complete.
        if self.shutdown_signaled.swap(true, Ordering::AcqRel) {
            return cause;
        }

        let hooks = std::mem::take(&mut *self.retained_hooks.lock_result().expect(ERR_POISONED_HOOKS));
        for endpoint in &self.worker_endpoints {
            // A closed channel means that the worker no longer accepts commands.
            _ = endpoint.commands.system_sender().send(SystemCommand::Shutdown);
            endpoint.waker.wake_by_ref();
        }
        // Hook data can hold a Context or register hooks from its destructor.
        drop(hooks);
        cause
    }

    #[expect(clippy::arithmetic_side_effects, reason = "impossible to divide by zero due to NonEmpty")]
    fn next_worker_index(&self) -> WorkerIndex {
        // This counter only chooses a worker; it does not publish any worker state.
        WorkerIndex(self.next_async_worker_index.fetch_add(1, Ordering::Relaxed) % self.worker_endpoints.len())
    }

    pub(crate) fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue(self.next_worker_index(), TaskPlacement::Any, future_factory)
    }

    pub(crate) fn spawn_relocating<FF, F, R>(&self, source: &Thread, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        let source_index = self
            .worker_index(source.id())
            .expect("a scheduler source must identify a registered runtime worker");
        let worker_index = self.next_worker_index();
        let endpoint = &self.worker_endpoints[usize::from(worker_index)];
        let future_factory = if &endpoint.thread == source {
            if let Some(join_handle) = JoinHandle::try_from_shutdown(self.shutdown()) {
                return join_handle;
            }
            // Only inspect local state when round-robin already chose the bound
            // worker. A retained scheduler can still be used from another thread.
            match LocalWorker::with_scheduler(|current| {
                let Some(current) = current.filter(|current| current.thread() == &endpoint.thread) else {
                    return Err(future_factory);
                };
                let handle = current.spawn(future_factory, TaskPlacement::Any);
                if !current.tasks().is_executing() {
                    endpoint.waker.wake_by_ref();
                }
                Ok(handle)
            }) {
                Ok(handle) => return handle.relocate_result_from(endpoint.thread.clone()),
                Err(future_factory) => future_factory,
            }
        } else {
            future_factory
        };
        self.spawn_relocating_on_worker(source_index, worker_index, TaskPlacement::Any, future_factory)
    }

    pub(crate) fn spawn_at_worker_relocating<FF, F, R>(
        &self,
        source_index: WorkerIndex,
        worker_index: WorkerIndex,
        future_factory: FF,
    ) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        self.spawn_relocating_on_worker(source_index, worker_index, TaskPlacement::SpecificThread, future_factory)
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

    pub(crate) fn access_io(&self, worker_index: WorkerIndex, access: Box<dyn FnOnce(&Context) + Send>) -> Result<(), Error> {
        if matches!(self.shutdown().state(), ShutdownState::Started) {
            return Err(self.runtime_shutting_down_error());
        }
        let endpoint = self
            .worker_endpoints
            .get(usize::from(worker_index))
            .expect("worker index must identify a registered runtime worker");
        let send_result = endpoint.commands.system_sender().send(SystemCommand::AccessIo {
            access: QueuedIoAccess::new(access),
        });
        endpoint.waker.wake_by_ref();
        send_result.map_err(|_error| self.runtime_shutting_down_error())
    }

    pub(crate) fn register_hook(&self, hook: HookRegistration) {
        if matches!(self.shutdown().state(), ShutdownState::Started) {
            return;
        }

        let source = LocalWorker::current_thread().filter(|thread| self.owns(thread));
        if let Some(source) = &source {
            LocalWorker::install_hook(hook.clone(), Some(source));
        }
        for endpoint in &self.worker_endpoints {
            if source.as_ref().is_some_and(|source| source == &endpoint.thread) {
                continue;
            }
            _ = endpoint.commands.system_sender().send(SystemCommand::RegisterHook {
                hook: hook.clone(),
                source: source.clone(),
            });
            endpoint.waker.wake_by_ref();
        }
        self.retain_hook(hook);
    }

    fn retain_hook(&self, hook: HookRegistration) {
        if !hook.is_retained() {
            return;
        }
        let mut retained = self.retained_hooks.lock_result().expect(ERR_POISONED_HOOKS);
        if matches!(self.shutdown().state(), ShutdownState::Running) {
            retained.push(hook);
        }
    }

    fn runtime_shutting_down_error(&self) -> Error {
        Error::from_shutdown(self.shutdown()).unwrap_or_else(Error::runtime_shutting_down)
    }

    pub(crate) const fn shutdown(&self) -> &Shutdown {
        self.remote_results.shutdown()
    }

    pub(crate) fn next_blocking_worker(&self) -> Arc<Worker> {
        self.blocking_worker(self.next_worker_index())
    }

    pub(crate) fn blocking_worker(&self, worker_index: WorkerIndex) -> Arc<Worker> {
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

    #[cfg(test)]
    pub(crate) fn spawn_on_worker<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue(worker_index, TaskPlacement::SameThread, future_factory)
    }

    pub(crate) fn spawn_on_worker_from<FF, F, R>(
        &self,
        source_index: WorkerIndex,
        worker_index: WorkerIndex,
        future_factory: FF,
    ) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.enqueue_from_worker(source_index, worker_index, TaskPlacement::SameThread, future_factory)
    }

    /// Records the execution worker so a successful result can relocate when its join is polled.
    fn spawn_relocating_on_worker<FF, F, R>(
        &self,
        source_index: WorkerIndex,
        worker_index: WorkerIndex,
        placement: TaskPlacement,
        future_factory: FF,
    ) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        let source = self.worker_endpoints[usize::from(worker_index)].thread.clone();
        self.enqueue_from_worker(source_index, worker_index, placement, future_factory)
            .relocate_result_from(source)
    }

    fn enqueue<FF, F, R>(&self, worker_index: WorkerIndex, placement: TaskPlacement, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        if let Some(join_handle) = JoinHandle::try_from_shutdown(self.shutdown()) {
            drop(future_factory);
            return join_handle;
        }
        let (future_factory, join_handle) = prepare_remote(&self.remote_results, future_factory, placement);
        let Some(spawn) = self.neutral_spawn() else {
            return join_handle;
        };
        self.send_prepared(worker_index, future_factory.with_spawn(spawn));

        join_handle
    }

    fn enqueue_from_worker<FF, F, R>(
        &self,
        source_index: WorkerIndex,
        worker_index: WorkerIndex,
        placement: TaskPlacement,
        future_factory: FF,
    ) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        if let Some(join_handle) = JoinHandle::try_from_shutdown(self.shutdown()) {
            drop(future_factory);
            return join_handle;
        }
        let (future_factory, join_handle) = prepare_remote(&self.remote_results, future_factory, placement);
        let source = &self.worker_endpoints[usize::from(source_index)];
        if LocalWorker::current_thread().is_some_and(|thread| thread == source.thread) {
            let Some(spawn) = LocalWorker::on_spawn() else {
                return join_handle;
            };
            self.send_prepared(worker_index, future_factory.with_spawn(spawn));
            return join_handle;
        }
        let Some(spawn) = self.neutral_spawn() else {
            return join_handle;
        };
        self.send_prepared(worker_index, future_factory.with_spawn(spawn));
        join_handle
    }

    fn neutral_spawn(&self) -> Option<Spawn> {
        if matches!(self.shutdown().state(), ShutdownState::Started) {
            return None;
        }
        let mut event = Spawn::new(None);
        for hook in &mut *self.retained_hooks.lock_result().expect(ERR_POISONED_HOOKS) {
            if let HookRegistration::Spawn(hook) = hook {
                hook.execute(&mut event);
            }
        }
        matches!(self.shutdown().state(), ShutdownState::Running).then_some(event)
    }

    fn send_prepared(&self, worker_index: WorkerIndex, future_factory: RemoteFutureFactory) {
        let endpoint = &self.worker_endpoints[usize::from(worker_index)];
        // A worker can close its channel before this send if shutdown already
        // reached that worker. The join handle detects the disconnection.
        _ = endpoint.commands.spawn_sender().send(SpawnCommand {
            future_factory: QueuedFactory::new(future_factory),
        });
        endpoint.waker.wake_by_ref();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
// These checks inject inconsistent endpoints and inspect commands before any worker runs.
mod tests {
    use std::error::Error as _;
    use std::pin::pin;
    use std::task;
    use std::task::Poll;

    use arty_executor::ShutdownError;

    use super::*;
    use crate::runtime::config::RuntimeName;
    use crate::runtime::dispatch::{DispatcherClient, test_threads};
    use crate::runtime::hook::{HookFn, Panic, WorkerInit, registration};
    use crate::runtime::worker::asynchronous::{CommandReceiver, command_channel};
    use crate::runtime::worker::blocking::Pool;

    fn endpoint(thread: &Thread) -> (WorkerEndpoint, CommandReceiver) {
        let (commands, command_rx) = command_channel();
        (
            WorkerEndpoint {
                commands,
                waker: Waker::noop().clone(),
                thread: thread.clone(),
                blocking_worker: Worker::new(Pool::new(RuntimeName::default(), None, None), Shutdown::default()),
            },
            command_rx,
        )
    }

    #[test]
    fn dispatcher_retains_only_hooks_that_require_retention() {
        let threads = test_threads(1);
        let (worker, _worker_rx) = endpoint(&threads[0]);
        let worker_init = registration(HookFn::<WorkerInit, _>::new((), |_event, ()| {}));
        let panic = registration(HookFn::<Panic, _>::new((), |_event, ()| {}));
        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), vec![worker_init.clone(), panic.clone()]);

        assert_eq!(dispatcher.retained_hooks.lock_result().unwrap().len(), 1);
        dispatcher.register_hook(worker_init.clone());
        dispatcher.register_hook(panic);
        assert_eq!(dispatcher.retained_hooks.lock_result().unwrap().len(), 2);
        dispatcher.register_hook(worker_init);
        assert_eq!(dispatcher.retained_hooks.lock_result().unwrap().len(), 3);
    }

    struct RegisterOnClone(Arc<DispatcherCore>);

    impl Clone for RegisterOnClone {
        fn clone(&self) -> Self {
            self.0
                .register_hook(registration(HookFn::<WorkerInit, _>::new((), |_event, ()| {})));
            Self(self.0.clone())
        }
    }

    impl ThreadAware for RegisterOnClone {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[test]
    fn retained_hook_allows_registration_from_clone() {
        let threads = test_threads(1);
        let (worker, _worker_rx) = endpoint(&threads[0]);
        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());
        let hook = registration(HookFn::<WorkerInit, _>::new(
            RegisterOnClone(dispatcher.clone()),
            |_event, _data| {},
        ));

        dispatcher.register_hook(hook);
        assert_eq!(dispatcher.retained_hooks.lock_result().unwrap().len(), 2);

        dispatcher.stop_inner(ShutdownCause::default());
        dispatcher.retain_hook(registration(HookFn::<WorkerInit, _>::new((), |_event, ()| {})));
        assert!(dispatcher.retained_hooks.lock_result().unwrap().is_empty());
    }

    #[test]
    #[should_panic(expected = "same runtime owner")]
    fn dispatcher_rejects_mixed_runtime_owners() {
        let first = test_threads(1).remove(0);
        let second = test_threads(1).remove(0);

        DispatcherCore::new(
            NonEmpty::from_vec(vec![endpoint(&first).0, endpoint(&second).0]).unwrap(),
            Shutdown::default(),
            Vec::new(),
        );
    }

    #[test]
    fn dispatch_spawn_single() {
        // We dispatch two tasks to the only worker.

        let threads = test_threads(1);
        let (worker, worker_rx) = endpoint(&threads[0]);

        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        assert_eq!(drain_spawn(&worker_rx).len(), 2);
    }

    #[test]
    fn dispatch_spawn_two() {
        // Dispatch one task to each worker. This test depends on the current
        // round-robin implementation. If the scheduling strategy changes, move
        // this expectation to a test for that strategy.

        let threads = test_threads(2);
        let (worker1, worker1_rx) = endpoint(&threads[0]);
        let (worker2, worker2_rx) = endpoint(&threads[1]);

        let dispatcher = DispatcherCore::new(NonEmpty::from_vec(vec![worker1, worker2]).unwrap(), Shutdown::default(), Vec::new());

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });

        assert_eq!(drain_spawn(&worker1_rx).len(), 1);
        assert_eq!(drain_spawn(&worker2_rx).len(), 1);
    }

    #[test]
    fn round_robin_repeats_without_resetting() {
        let threads = test_threads(2);
        let (worker1, worker1_rx) = endpoint(&threads[0]);
        let (worker2, worker2_rx) = endpoint(&threads[1]);

        let dispatcher = DispatcherCore::new(NonEmpty::from_vec(vec![worker1, worker2]).unwrap(), Shutdown::default(), Vec::new());

        for _ in 0..4 {
            dispatcher.spawn(|_| async move { unreachable!("we do not expect the task to be executed") });
        }

        assert_eq!(drain_spawn(&worker1_rx).len(), 2);
        assert_eq!(drain_spawn(&worker2_rx).len(), 2);
    }

    #[test]
    fn same_thread_placement_selects_matching_worker() {
        let threads = test_threads(2);
        let (first, first_rx) = endpoint(&threads[0]);
        let (second, second_rx) = endpoint(&threads[1]);
        let dispatcher = DispatcherCore::new(NonEmpty::from_vec(vec![first, second]).unwrap(), Shutdown::default(), Vec::new());

        let index = dispatcher.worker_index(threads[1].id()).unwrap();
        dispatcher.spawn_on_worker(index, |_| async {});

        assert_eq!((drain_spawn(&first_rx).len(), drain_spawn(&second_rx).len()), (0, 1),);
    }

    #[test]
    fn unregistered_thread_has_no_worker_index() {
        let threads = test_threads(2);
        let dispatcher = DispatcherCore::new(NonEmpty::new(endpoint(&threads[0]).0), Shutdown::default(), Vec::new());

        assert!(dispatcher.worker_index(threads[1].id()).is_none());
    }

    #[test]
    fn shutdown_state_is_shared_with_dispatcher_client() {
        let threads = test_threads(1);
        let core = DispatcherCore::new(NonEmpty::new(endpoint(&threads[0]).0), Shutdown::default(), Vec::new());
        let client = DispatcherClient::new(Arc::clone(&core));
        let shutdown = client.shutdown();

        core.stop_inner(ShutdownCause::default());

        assert_eq!(shutdown.state(), ShutdownState::Started);
    }

    #[test]
    fn io_access_after_shutdown_is_rejected() {
        let threads = test_threads(1);
        let (worker, rx) = endpoint(&threads[0]);
        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());
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
        assert!(matches!(drain_system(&rx).as_slice(), [SystemCommand::Shutdown]));
    }

    #[test]
    fn worker_failure_is_attached_to_rejected_task_joins() {
        let threads = test_threads(1);
        let dispatcher = DispatcherCore::new(NonEmpty::new(endpoint(&threads[0]).0), Shutdown::default(), Vec::new());
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
        let threads = test_threads(2);
        let (worker1, worker1_rx) = endpoint(&threads[0]);
        let (worker2, worker2_rx) = endpoint(&threads[1]);

        let dispatcher = DispatcherCore::new(NonEmpty::from_vec(vec![worker1, worker2]).unwrap(), Shutdown::default(), Vec::new());

        assert_eq!(drain_system(&worker1_rx).len(), 0);
        assert_eq!(drain_system(&worker2_rx).len(), 0);

        dispatcher.stop_inner(ShutdownCause::default());

        assert_eq!(drain_system(&worker1_rx).len(), 1);
        assert_eq!(drain_system(&worker2_rx).len(), 1);

        // A second stop call sends no more stop commands.
        dispatcher.stop_inner(ShutdownCause::default());

        assert_eq!(drain_system(&worker1_rx).len(), 0);
        assert_eq!(drain_system(&worker2_rx).len(), 0);
    }

    #[test]
    fn spawn_after_command_channel_closed_returns_disconnected_join_handle() {
        // After a worker closes its command channel during shutdown, later
        // spawn requests return disconnected join handles.

        let threads = test_threads(1);
        let (worker, command_rx) = endpoint(&threads[0]);

        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());

        dispatcher.stop_inner(ShutdownCause::default());

        // Drain the shutdown command.
        assert!(matches!(command_rx.system_receiver().try_recv(), Ok(SystemCommand::Shutdown)));

        // Close the command channel.
        drop(command_rx);

        let join_handle = dispatcher.spawn(|_| async move {});
        let join_handle = pin!(join_handle);

        let mut cx = task::Context::from_waker(Waker::noop());
        let Poll::Ready(Err(error)) = join_handle.poll(&mut cx) else {
            panic!("a disconnected join must report shutdown");
        };
        assert!(error.is_shutdown());
    }

    fn shutdown_dispatcher() -> (Arc<DispatcherCore>, DispatcherClient, Shutdown) {
        let thread = test_threads(1).remove(0);
        let shutdown = Shutdown::default();
        let core = DispatcherCore::new(NonEmpty::new(endpoint(&thread).0), shutdown.clone(), Vec::new());
        let client = DispatcherClient::new(Arc::clone(&core));
        (core, client, shutdown)
    }

    #[test]
    fn published_shutdown_cause_is_shared_with_dispatcher_and_client() {
        let (core, client, shutdown) = shutdown_dispatcher();
        let cause = ShutdownCause::panic(&"task panic");
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

    fn drain_system(worker_rx: &CommandReceiver) -> Vec<SystemCommand> {
        let mut commands = Vec::new();
        commands.extend(std::iter::from_fn(|| worker_rx.system_receiver().try_recv().ok()));
        commands
    }

    fn drain_spawn(worker_rx: &CommandReceiver) -> Vec<SpawnCommand> {
        let mut commands = Vec::new();
        commands.extend(std::iter::from_fn(|| worker_rx.spawn_receiver().try_recv().ok()));
        commands
    }
}
