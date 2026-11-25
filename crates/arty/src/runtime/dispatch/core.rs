// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared worker routing and shutdown state.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Waker;
use std::thread::ThreadId;

use arty_executor::ShutdownCause;
use foldhash::HashMap;
use nonempty::NonEmpty;
use performables::arc::Arc;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::hook::{HookRegistration, RetainedHooks, TaskObservers, TaskSpawn};
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::worker::asynchronous::{CommandSender, LocalWorker, QueuedFactory, QueuedIoAccess, SpawnCommand, SystemCommand};
use crate::runtime::worker::blocking::Worker;
use crate::runtime::{Error, Workers};
use crate::task::Context;
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::{RemoteFutureFactory, TaskSpawnOptions, prepare_remote};

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
    workers: StdArc<[Thread]>,

    // Immutable, runtime-local lookup for placement and relocation; ordinary spawn uses a
    // cached index. IDs come only from registered workers, so a non-cryptographic hasher fits.
    worker_indices: HashMap<ThreadId, WorkerIndex>,

    retained_hooks: RetainedHooks,

    // Minimal effort round-robin scheduling of async tasks.
    next_async_worker_index: AtomicUsize,

    remote_results: JoinHandlePool,
    #[cfg(any(test, feature = "monitor"))]
    diagnostics: std::sync::OnceLock<seismograph_runtime::RuntimeHandle>,
}

impl DispatcherCore {
    pub(crate) fn new(
        worker_endpoints: NonEmpty<WorkerEndpoint>,
        shutdown: Shutdown,
        hook_registrations: Vec<HookRegistration>,
    ) -> Arc<Self> {
        // #[gamma::skip(iter.first_to_last, reason = "The following assertion requires every endpoint to have the same owner.")]
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
        let workers = worker_endpoints
            .iter()
            .map(|endpoint| endpoint.thread.clone())
            .collect::<Vec<_>>()
            .into();
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
            retained_hooks: RetainedHooks::new(hook_registrations),
            next_async_worker_index: AtomicUsize::new(0),
            remote_results: JoinHandlePool::new(shutdown),
            #[cfg(any(test, feature = "monitor"))]
            diagnostics: std::sync::OnceLock::new(),
        })
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn set_diagnostics(&self, runtime: Option<seismograph_runtime::RuntimeHandle>) {
        if let Some(runtime) = runtime {
            self.diagnostics
                .set(runtime)
                .expect("bootstrap sets runtime diagnostics exactly once");
        }
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn register_task_provider(&self, thread: ThreadId, handle: &seismograph_runtime::worker::WorkerHandle) {
        let index = self
            .worker_index(thread)
            .expect("worker diagnostics use a registered runtime thread");
        let endpoint = &self.worker_endpoints[usize::from(index)];
        let commands = endpoint.commands.system_sender().clone();
        let waker = endpoint.waker.clone();
        handle
            .register_task_provider(move |request| {
                if commands.send(SystemCommand::Inspect { request }).is_ok() {
                    waker.wake_by_ref();
                }
                // A failed send drops the request and reports collection failure.
            })
            .expect("bootstrap registers providers before their workers can stop");
    }

    pub(crate) fn stop_inner(&self, cause: ShutdownCause) -> ShutdownCause {
        self.shutdown().begin(cause);
        #[cfg(any(test, feature = "monitor"))]
        if let Some(runtime) = self.diagnostics.get() {
            runtime.stopping();
        }
        let cause = self.shutdown().cause().unwrap_or_default();
        // Shutdown stores the cause independently. This atomic only elects the
        // broadcaster. Observing `true` does not mean notification is complete.
        if self.shutdown_signaled.swap(true, Ordering::AcqRel) {
            return cause;
        }

        for endpoint in &self.worker_endpoints {
            // A closed channel means that the worker no longer accepts commands.
            _ = endpoint.commands.system_sender().send(SystemCommand::Shutdown);
            endpoint.waker.wake_by_ref();
        }
        cause
    }

    pub(crate) fn release_retained_hooks(&self) {
        let hooks = self.retained_hooks.clear();
        // Hook data can hold a Context or register hooks from its destructor.
        // Runtime shutdown calls this only after every worker has stopped.
        drop(hooks);
    }

    #[expect(clippy::arithmetic_side_effects, reason = "impossible to divide by zero due to NonEmpty")]
    fn next_worker_index(&self) -> WorkerIndex {
        // This counter only chooses a worker; it does not publish any worker state.
        WorkerIndex(self.next_async_worker_index.fetch_add(1, Ordering::Relaxed) % self.worker_endpoints.len())
    }

    pub(crate) fn spawn<FF, F>(&self, future_factory: FF, options: TaskSpawnOptions) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        self.enqueue(self.next_worker_index(), future_factory, options)
    }

    pub(crate) fn spawn_relocating<FF, F>(&self, source: &Thread, future_factory: FF) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: ThreadAware + 'static,
    {
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
                let handle = current.spawn(future_factory);
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
        self.spawn_relocating_on_worker(worker_index, future_factory)
    }

    pub(crate) fn spawn_at_worker_relocating<FF, F>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: ThreadAware + 'static,
    {
        self.spawn_relocating_on_worker(worker_index, future_factory)
    }

    pub(crate) fn worker_index(&self, thread_id: ThreadId) -> Option<WorkerIndex> {
        self.worker_indices.get(&thread_id).copied()
    }

    pub(crate) fn workers(&self) -> Workers {
        Workers::new(StdArc::clone(&self.workers))
    }

    pub(crate) fn owns(&self, thread: &Thread) -> bool {
        // #[gamma::skip(iter.first_to_last, reason = "Construction asserts that every endpoint has the same owner.")]
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
        // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        endpoint.waker.wake_by_ref();
        send_result.map_err(|_error| self.runtime_shutting_down_error())
    }

    pub(crate) fn register_hook(&self, hook: HookRegistration) {
        if matches!(self.shutdown().state(), ShutdownState::Started) {
            return;
        }

        // Clone before taking the retention lock because application Clone code
        // can register another hook.
        let delivery = hook.clone();
        // Retain the caller's original before a callback runs or a command can
        // reach another worker. retain_hook releases the lock before delivery.
        if hook.is_retained() && !self.retain_hook(hook) {
            return;
        }
        let hook = delivery;
        let source = LocalWorker::current_thread().filter(|thread| self.owns(thread));
        let mut remote = self
            .worker_endpoints
            .iter()
            .filter(|endpoint| source.as_ref().is_none_or(|source| source != &endpoint.thread))
            .peekable();
        if let Some(source) = &source {
            if remote.peek().is_none() {
                LocalWorker::install_hook(hook, Some(source));
                return;
            }
            LocalWorker::install_hook(hook.clone(), Some(source));
        }
        while let Some(endpoint) = remote.next() {
            // The prepared clone becomes the final worker's data. Earlier
            // workers receive clones, so application Clone runs once per worker.
            if remote.peek().is_none() {
                _ = endpoint.commands.system_sender().send(SystemCommand::RegisterHook {
                    hook,
                    source: source.clone(),
                });
                endpoint.waker.wake_by_ref();
                break;
            }
            _ = endpoint.commands.system_sender().send(SystemCommand::RegisterHook {
                hook: hook.clone(),
                source: source.clone(),
            });
            endpoint.waker.wake_by_ref();
        }
    }

    fn retain_hook(&self, hook: HookRegistration) -> bool {
        if matches!(self.shutdown().state(), ShutdownState::Started) {
            return false;
        }
        self.retained_hooks.push(hook)
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

    pub(crate) fn spawn_on_worker<FF, F>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        self.enqueue(worker_index, future_factory, TaskSpawnOptions::new::<F>())
    }

    /// Records the execution worker so a successful result can relocate when its join is polled.
    fn spawn_relocating_on_worker<FF, F>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: ThreadAware + 'static,
    {
        let source = self.worker_endpoints[usize::from(worker_index)].thread.clone();
        self.enqueue(worker_index, future_factory, TaskSpawnOptions::new::<F>())
            .relocate_result_from(source)
    }

    fn enqueue<FF, F>(&self, worker_index: WorkerIndex, future_factory: FF, options: TaskSpawnOptions) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        if let Some(join_handle) = JoinHandle::try_from_shutdown(self.shutdown()) {
            drop(future_factory);
            return join_handle;
        }
        let target = &self.worker_endpoints[usize::from(worker_index)].thread;
        let options = options.observed_by(self.submission_observers(target));
        let (future_factory, join_handle) = prepare_remote(&self.remote_results, future_factory, options);
        self.send_prepared(worker_index, future_factory);
        join_handle
    }

    fn submission_observers(&self, target: &Thread) -> Option<TaskObservers> {
        // #[gamma::skip(cond.always_false, reason = "This guard only avoids hook discovery; the empty hook snapshot also returns no observers.")]
        if !self.retained_hooks.has_task_spawn() {
            return None;
        }
        if LocalWorker::current_thread().is_some_and(|thread| self.owns(&thread)) {
            LocalWorker::on_spawn(target)
        } else {
            self.retained_hooks.execute_task_spawn(|| TaskSpawn::new(None, target.clone()))
        }
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
    use crate::runtime::hook::{HookFn, TaskPanic, WorkerSpawn, registration};
    use crate::runtime::worker::asynchronous::{CommandReceiver, command_channel};
    use crate::runtime::worker::blocking::Pool;

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl std::task::Wake for WakeCounter {
        fn wake(self: StdArc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &StdArc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn gamma_survivor_private_dispatch_starts_at_worker_zero_and_wakes_io_destination() {
        let threads = test_threads(3);
        let counter = StdArc::new(WakeCounter::default());
        let (mut first, rx) = endpoint(&threads[0]);
        first.waker = Waker::from(StdArc::clone(&counter));
        let (second, _second_rx) = endpoint(&threads[1]);
        let (third, _third_rx) = endpoint(&threads[2]);
        let core = DispatcherCore::new(
            NonEmpty::from_vec(vec![first, second, third]).unwrap(),
            Shutdown::default(),
            Vec::new(),
        );
        let selected: Vec<_> = (0..7).map(|_| usize::from(core.next_worker_index())).collect();
        assert_eq!(selected, [0, 1, 2, 0, 1, 2, 0]);
        core.access_io(WorkerIndex(0), Box::new(|_| {})).unwrap();
        assert!(matches!(rx.system_receiver().try_recv().unwrap(), SystemCommand::AccessIo { .. }));
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        let client = DispatcherClient::new(core);
        assert_eq!(format!("{client:?}"), std::any::type_name::<DispatcherClient>());
    }

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

    fn spawn_for_test<FF, F>(dispatcher: &DispatcherCore, future_factory: FF) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        dispatcher.spawn(future_factory, TaskSpawnOptions::new::<F>())
    }

    #[test]
    fn dispatcher_retains_only_hooks_that_require_retention() {
        let threads = test_threads(1);
        let (worker, _worker_rx) = endpoint(&threads[0]);
        let worker_spawn = registration(HookFn::<WorkerSpawn, _>::new((), |_event, ()| {}));
        let panic = registration(HookFn::<TaskPanic, _>::new((), |_event, ()| {}));
        let dispatcher = DispatcherCore::new(
            NonEmpty::new(worker),
            Shutdown::default(),
            vec![worker_spawn.clone(), panic.clone()],
        );

        assert_eq!(dispatcher.retained_hooks.len(), 1);
        dispatcher.register_hook(worker_spawn.clone());
        dispatcher.register_hook(panic);
        assert_eq!(dispatcher.retained_hooks.len(), 2);
        dispatcher.register_hook(worker_spawn);
        assert_eq!(dispatcher.retained_hooks.len(), 3);
    }

    struct RegisterOnClone(Arc<DispatcherCore>);

    impl Clone for RegisterOnClone {
        fn clone(&self) -> Self {
            self.0
                .register_hook(registration(HookFn::<WorkerSpawn, _>::new((), |_event, ()| {})));
            Self(self.0.clone())
        }
    }

    impl ThreadAware for RegisterOnClone {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    struct StopOnClone(Arc<DispatcherCore>);

    impl Clone for StopOnClone {
        fn clone(&self) -> Self {
            self.0.stop_inner(ShutdownCause::default());
            Self(self.0.clone())
        }
    }

    impl ThreadAware for StopOnClone {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[test]
    fn retained_hook_allows_registration_from_clone() {
        let threads = test_threads(1);
        let (worker, _worker_rx) = endpoint(&threads[0]);
        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());
        let hook = registration(HookFn::<WorkerSpawn, _>::new(
            RegisterOnClone(dispatcher.clone()),
            |_event, _data| {},
        ));

        dispatcher.register_hook(hook);
        assert_eq!(dispatcher.retained_hooks.len(), 2);

        dispatcher.stop_inner(ShutdownCause::default());
        assert!(!dispatcher.retain_hook(registration(HookFn::<WorkerSpawn, _>::new((), |_event, ()| {}))));
        assert_eq!(dispatcher.retained_hooks.len(), 2);
        dispatcher.release_retained_hooks();
        assert_eq!(dispatcher.retained_hooks.len(), 0);
    }

    #[test]
    fn shutdown_from_clone_prevents_retention_and_delivery() {
        let threads = test_threads(1);
        let (worker, worker_rx) = endpoint(&threads[0]);
        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());
        let hook = registration(HookFn::<WorkerSpawn, _>::new(StopOnClone(dispatcher.clone()), |_event, _data| {}));

        dispatcher.register_hook(hook);

        assert_eq!(dispatcher.retained_hooks.len(), 0);
        assert!(matches!(worker_rx.system_receiver().try_recv().unwrap(), SystemCommand::Shutdown));
        worker_rx.system_receiver().try_recv().unwrap_err();
    }

    #[test]
    fn panic_hook_is_queued_before_its_original_capture_can_stop_the_runtime() {
        struct StopOnOriginalDrop(Option<Arc<DispatcherCore>>);

        impl Clone for StopOnOriginalDrop {
            fn clone(&self) -> Self {
                Self(None)
            }
        }

        impl Drop for StopOnOriginalDrop {
            fn drop(&mut self) {
                if let Some(dispatcher) = &self.0 {
                    dispatcher.stop_inner(ShutdownCause::default());
                }
            }
        }

        impl ThreadAware for StopOnOriginalDrop {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }

        let threads = test_threads(1);
        let (worker, worker_rx) = endpoint(&threads[0]);
        let dispatcher = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());
        // TaskPanic hooks are not retained, and their cloned data is inert. Only dropping
        // the original can stop the dispatcher, so the command order tests registration.
        let hook = HookFn::<TaskPanic, _>::new(StopOnOriginalDrop(Some(dispatcher.clone())), |_event, _data| {});

        dispatcher.register_hook(registration(hook));

        let receiver = worker_rx.system_receiver();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            SystemCommand::RegisterHook {
                hook: HookRegistration::TaskPanic(_),
                ..
            }
        ));
        assert!(matches!(receiver.try_recv().unwrap(), SystemCommand::Shutdown));
        receiver.try_recv().unwrap_err();
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

        spawn_for_test(
            &dispatcher,
            |_| async move { unreachable!("we do not expect the task to be executed") },
        );

        spawn_for_test(
            &dispatcher,
            |_| async move { unreachable!("we do not expect the task to be executed") },
        );

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

        spawn_for_test(
            &dispatcher,
            |_| async move { unreachable!("we do not expect the task to be executed") },
        );

        spawn_for_test(
            &dispatcher,
            |_| async move { unreachable!("we do not expect the task to be executed") },
        );

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
            spawn_for_test(
                &dispatcher,
                |_| async move { unreachable!("we do not expect the task to be executed") },
            );
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

        let mut join = pin!(spawn_for_test(&dispatcher, |_| async {}));
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
    fn dispatcher_stop_sends_one_stop_command_and_wakes_each_worker_once() {
        let threads = test_threads(2);
        let (mut worker1, worker1_rx) = endpoint(&threads[0]);
        let (mut worker2, worker2_rx) = endpoint(&threads[1]);
        let wake1 = StdArc::new(WakeCounter::default());
        let wake2 = StdArc::new(WakeCounter::default());
        worker1.waker = Waker::from(StdArc::clone(&wake1));
        worker2.waker = Waker::from(StdArc::clone(&wake2));

        let dispatcher = DispatcherCore::new(NonEmpty::from_vec(vec![worker1, worker2]).unwrap(), Shutdown::default(), Vec::new());

        assert_eq!(drain_system(&worker1_rx).len(), 0);
        assert_eq!(drain_system(&worker2_rx).len(), 0);

        dispatcher.stop_inner(ShutdownCause::default());

        assert_eq!(drain_system(&worker1_rx).len(), 1);
        assert_eq!(drain_system(&worker2_rx).len(), 1);
        // The dispatcher invokes these wakers inline. The counters publish no other state.
        assert_eq!(wake1.0.load(Ordering::Relaxed), 1);
        assert_eq!(wake2.0.load(Ordering::Relaxed), 1);

        // A second stop call neither sends commands nor wakes workers.
        dispatcher.stop_inner(ShutdownCause::default());

        assert_eq!(drain_system(&worker1_rx).len(), 0);
        assert_eq!(drain_system(&worker2_rx).len(), 0);
        assert_eq!(wake1.0.load(Ordering::Relaxed), 1);
        assert_eq!(wake2.0.load(Ordering::Relaxed), 1);
    }

    #[cfg(any(test, feature = "monitor"))]
    #[test]
    fn dispatcher_stop_updates_its_registered_runtime() {
        use seismograph_runtime::snapshot::{CaptureOptions, RuntimeState};

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("dispatcher-stop", 1));
        let threads = test_threads(1);
        let (worker, _worker_rx) = endpoint(&threads[0]);
        let core = DispatcherCore::new(NonEmpty::new(worker), Shutdown::default(), Vec::new());
        let client = DispatcherClient::new(core);
        client.set_diagnostics(None);
        client.set_diagnostics(Some(runtime.handle()));

        client.stop(ShutdownCause::default());

        assert_eq!(
            runtime.handle().capture(CaptureOptions::default()).unwrap().state,
            RuntimeState::Stopping
        );
    }

    #[cfg(any(test, feature = "monitor"))]
    #[test]
    fn a_closed_worker_command_channel_reports_collection_failure() {
        use seismograph_runtime::snapshot::{CaptureOptions, TaskCollection};
        use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("closed-worker-channel", 1));
        let threads = test_threads(1);
        let (endpoint, receiver) = endpoint(&threads[0]);
        let dispatcher = DispatcherCore::new(NonEmpty::new(endpoint), Shutdown::default(), Vec::new());
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        dispatcher.register_task_provider(threads[0].id(), &worker.handle());
        drop(receiver);

        let snapshot = runtime.handle().capture(CaptureOptions::default()).unwrap();
        assert!(snapshot.tasks.is_empty());
        assert_eq!(snapshot.workers[0].task_collection, TaskCollection::Failed);
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

        let join_handle = spawn_for_test(&dispatcher, |_| async move {});
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
