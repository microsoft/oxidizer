// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use arty_executor::{Shutdown as ExecutorShutdown, ShutdownCause, TaskSet};
use seismograph_runtime::TaskIdentity;
use thread_aware::{Thread, ThreadAware};

use crate::io::RuntimeDriverHost;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::hook::{
    HookRegistration, LocalHook, LocalHookGuard, LocalHookStorage, TaskObservers, TaskPanic, TaskSpawn, WorkerSpawn,
};
use crate::runtime::worker::{self, WorkerMarker};
use crate::task::Context;
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::{DeferredTaskFactory, SpawnOptions};

thread_local! {
    static CURRENT: RefCell<Option<LocalWorker>> = const { RefCell::new(None) };
    static SPAWN_HOOKS: LocalHookStorage<TaskSpawn> = const { RefCell::new(None) };
    static PANIC_HOOKS: LocalHookStorage<TaskPanic> = const { RefCell::new(None) };
}

static LOCAL_SPAWN_HOOK: LocalHook<TaskSpawn> = LocalHook { hooks: &SPAWN_HOOKS };
static LOCAL_PANIC_HOOK: LocalHook<TaskPanic> = LocalHook { hooks: &PANIC_HOOKS };

/// Asynchronous worker state that remains attached throughout thread teardown.
#[derive(Debug)]
pub(crate) struct LocalWorker {
    monitoring: seismograph_runtime::asynchronous::Worker,
    scheduler: Option<SchedulerContext>,
    driver_host: Option<RuntimeDriverHost>,
    spawn_hooks: Option<LocalHookGuard<TaskSpawn>>,
    panic_hooks: Option<LocalHookGuard<TaskPanic>>,
}

impl LocalWorker {
    fn new(monitoring: seismograph_runtime::asynchronous::Worker) -> Self {
        Self {
            monitoring,
            scheduler: None,
            driver_host: None,
            spawn_hooks: None,
            panic_hooks: None,
        }
    }

    pub(crate) fn attach(monitoring: seismograph_runtime::asynchronous::Worker) {
        worker::WorkerMarker::attach();
        CURRENT.with_borrow_mut(|current| {
            assert!(current.is_none(), "a task local worker is already attached to this thread");
            *current = Some(Self::new(monitoring));
        });
    }

    pub(super) fn install_scheduler(
        context: Context,
        tasks: TaskSet,
        executor_shutdown: ExecutorShutdown,
        hook_registrations: Vec<HookRegistration>,
    ) {
        let results = JoinHandlePool::new(context.dispatcher().shutdown().clone());
        let scheduler = SchedulerContext {
            thread: context.thread().clone(),
            context,
            tasks,
            executor_shutdown,
            results,
            has_spawn_hooks: Cell::new(false),
        };
        let spawn_hooks = LOCAL_SPAWN_HOOK.initialize();
        let panic_hooks = LOCAL_PANIC_HOOK.initialize();
        Self::with_current_mut(|current| {
            let current = current.expect("task worker startup attaches its local worker before installing the scheduler");
            assert!(current.scheduler.is_none(), "a scheduler is already installed on this task worker");
            current.scheduler = Some(scheduler);
            current.spawn_hooks = Some(spawn_hooks);
            current.panic_hooks = Some(panic_hooks);
        });
        for hook in hook_registrations {
            Self::install_hook(hook, None);
        }
    }

    pub(crate) fn is_current() -> bool {
        CURRENT.try_with(|current| current.borrow().is_some()).unwrap_or(false)
    }

    pub(crate) fn current_thread() -> Option<Thread> {
        Self::with_scheduler(|current| current.map(|current| current.thread().clone()))
    }

    pub(crate) fn current_task() -> Option<TaskIdentity> {
        Self::with_scheduler(|current| current.and_then(|current| current.tasks().current()))
    }

    pub(crate) fn with_monitoring<R>(run: impl FnOnce(Option<&seismograph_runtime::asynchronous::Worker>) -> R) -> R {
        Self::with_current(|current| run(current.map(|current| &current.monitoring)))
    }

    pub(crate) fn with_scheduler<R>(f: impl FnOnce(Option<&SchedulerContext>) -> R) -> R {
        Self::with_current(|current| f(current.and_then(|current| current.scheduler.as_ref())))
    }

    pub(crate) fn begin_executor_shutdown(dispatcher: &DispatcherClient, cause: ShutdownCause) {
        _ = CURRENT.try_with(|current| {
            let current = current.borrow();
            let Some(current) = current.as_ref().and_then(|current| current.scheduler.as_ref()) else {
                return;
            };
            if dispatcher.owns(current.thread()) {
                current.executor_shutdown.trigger(cause);
            }
        });
    }

    pub(crate) fn on_spawn(target: &Thread, identity: TaskIdentity) -> Option<TaskObservers> {
        let origin = Self::with_scheduler(|current| {
            let current = current?;
            current.has_spawn_hooks.get().then(|| current.thread.clone())
        })?;
        Self::execute_spawn_hooks(&origin, target, identity)
    }

    fn execute_spawn_hooks(origin: &Thread, target: &Thread, identity: TaskIdentity) -> Option<TaskObservers> {
        let hooks = SPAWN_HOOKS.with_borrow(|hooks| Arc::clone(hooks.as_ref().expect("a running worker retains its installed hooks")));
        debug_assert!(!hooks.is_empty(), "the scheduler flag tracks installed spawn hooks");
        let mut event = TaskSpawn::new(Some(origin.clone()), target.clone(), identity);
        for hook in hooks.iter() {
            hook.execute(&mut event);
        }
        event.into_observers()
    }

    pub(crate) fn on_panic(panic: arty_executor::TaskPanicPayload) -> (arty_executor::PanicAction, arty_executor::TaskPanicPayload) {
        if !Self::with_scheduler(|current| current.is_some()) {
            return TaskPanic::new(panic).into_parts();
        }
        let mut event = TaskPanic::new(panic);
        LOCAL_PANIC_HOOK.execute(&mut event);
        event.into_parts()
    }

    pub(crate) fn install_hook(hook: HookRegistration, source: Option<&Thread>) {
        match hook {
            HookRegistration::TaskSpawn(hook) => {
                let destination = Self::with_scheduler(|current| {
                    let current = current.expect("a running worker retains its scheduler while processing hook registration");
                    current.thread.clone()
                });
                LOCAL_SPAWN_HOOK.install(hook, source, &destination);
                Self::with_scheduler(|current| {
                    current
                        .expect("a running worker retains its scheduler while processing hook registration")
                        .has_spawn_hooks
                        .set(true);
                });
            }
            HookRegistration::TaskPanic(hook) => {
                let destination = Self::with_scheduler(|current| {
                    let current = current.expect("a running worker retains its scheduler while processing hook registration");
                    current.thread.clone()
                });
                LOCAL_PANIC_HOOK.install(hook, source, &destination);
            }
            HookRegistration::WorkerSpawn(mut hook) => {
                let (destination, mut event) = Self::with_scheduler(|current| {
                    let current = current.expect("a running worker retains its scheduler while processing hook registration");
                    (current.thread.clone(), WorkerSpawn::new(current.thread.clone()))
                });
                hook.relocate(source, &destination);
                hook.execute(&mut event);
            }
        }
    }

    pub(super) fn clear_scheduler() {
        let (scheduler, spawn_hooks, panic_hooks) = Self::with_current_mut(|current| {
            current
                .map(|current| (current.scheduler.take(), current.spawn_hooks.take(), current.panic_hooks.take()))
                .unwrap_or_default()
        });
        drop(scheduler);
        drop(spawn_hooks);
        drop(panic_hooks);
    }

    pub(crate) fn install_driver_host(driver_host: RuntimeDriverHost) {
        Self::with_current_mut(|current| {
            let current = current.expect("task worker startup attaches its local worker before installing the I/O driver host");
            assert!(
                current.driver_host.is_none(),
                "an I/O driver host is already installed on this task worker"
            );
            current.driver_host = Some(driver_host);
        });
    }

    pub(crate) fn uninstall_driver_host() -> RuntimeDriverHost {
        Self::with_current_mut(|current| {
            let current = current.expect("task worker teardown retains its local worker while uninstalling the I/O driver host");
            current
                .driver_host
                .take()
                .expect("the task worker must uninstall the I/O driver host that it installed")
        })
    }

    pub(crate) fn with_driver_host<R>(f: impl FnOnce(&RuntimeDriverHost) -> R) -> R {
        Self::with_current(|current| current.and_then(|current| current.driver_host.as_ref()).map(f))
            .expect("the task worker must install its I/O driver host before running")
    }

    pub(crate) fn try_with_driver_host<R>(f: impl FnOnce(&RuntimeDriverHost) -> R) -> Option<R> {
        only_on_worker(WorkerMarker::is_current(), || {
            Self::with_current(|current| current.and_then(|current| current.driver_host.as_ref().map(f)))
        })
    }

    #[cfg(test)]
    pub(crate) fn has_scheduler_context() -> bool {
        Self::with_scheduler(|current| current.is_some())
    }

    fn with_current<R>(f: impl FnOnce(Option<&Self>) -> R) -> R {
        let mut f = Some(f);
        CURRENT
            .try_with(|current| {
                let current = current.borrow();
                f.take().expect("the local-worker callback is invoked exactly once")(current.as_ref())
            })
            .unwrap_or_else(|_| f.take().expect("the local-worker callback is invoked exactly once")(None))
    }

    fn with_current_mut<R>(f: impl FnOnce(Option<&mut Self>) -> R) -> R {
        CURRENT.with_borrow_mut(|current| f(current.as_mut()))
    }
}

fn only_on_worker<R>(current_worker: bool, access: impl FnOnce() -> Option<R>) -> Option<R> {
    if !current_worker {
        return None;
    }
    access()
}

#[cfg(test)]
mod gamma_round2_tests {
    #[test]
    fn gamma_round2_unmarked_driver_access_never_invokes_the_tls_accessor() {
        for marked in [false, true] {
            let mut accesses = 0;
            let result = super::only_on_worker(marked, || {
                accesses += 1;
                Some(59)
            });
            assert_eq!(result, marked.then_some(59));
            assert_eq!(accesses, usize::from(marked));
        }
    }
}

/// Task worker state available while direct task registration is safe.
#[derive(Debug)]
pub(crate) struct SchedulerContext {
    thread: Thread,
    context: Context,
    tasks: TaskSet,
    executor_shutdown: ExecutorShutdown,
    results: JoinHandlePool,
    has_spawn_hooks: Cell<bool>,
}

impl SchedulerContext {
    pub(crate) fn thread(&self) -> &Thread {
        &self.thread
    }

    pub(crate) fn tasks(&self) -> &TaskSet {
        &self.tasks
    }

    pub(crate) fn spawn<FF, F>(&self, future_factory: FF, options: SpawnOptions) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        if self.has_spawn_hooks.get() {
            return self.spawn_observed(future_factory, options);
        }

        let (handle, result_tx) = self.results.handle();
        let inner = DeferredTaskFactory::new(future_factory, self.context.clone());
        options.register(&self.thread, &self.tasks, inner, result_tx);
        handle
    }

    #[cold]
    fn spawn_observed<FF, F>(&self, future_factory: FF, options: SpawnOptions) -> JoinHandle<F::Output>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future + 'static,
        F::Output: Send + 'static,
    {
        let (handle, result_tx) = self.results.handle();
        let identity = options.identity();
        let observers = LocalWorker::execute_spawn_hooks(&self.thread, &self.thread, identity);
        let inner = DeferredTaskFactory::new(future_factory, self.context.clone());
        options.observed_by(observers).register(&self.thread, &self.tasks, inner, result_tx);
        handle
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
// Public tasks cannot install partial TLS registrations or inspect local queue routing.
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Poll, Wake, Waker};

    use arty_executor::Executor;
    use nonempty::NonEmpty;
    use performables::arc::Arc;
    use thread_aware::ThreadBuilder;
    use tick::runtime::InactiveClock;

    use super::*;
    use crate::runtime::config::RuntimeName;
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::hook::{HookFn, registration};
    use crate::runtime::shutdown::Shutdown;
    use crate::runtime::testing::worker_endpoint;
    use crate::runtime::worker::asynchronous::SpawnCommand;
    use crate::runtime::worker::blocking::{Pool, Worker};

    fn monitoring_worker() -> seismograph_runtime::asynchronous::Worker {
        seismograph_runtime::Runtime::builder("")
            .build()
            .async_worker(seismograph_runtime::WorkerMetadata::new(seismograph_runtime::WorkerRole::Core))
    }

    #[derive(Default)]
    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: std::sync::Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &std::sync::Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[derive(Clone)]
    struct ReentrantSpawnProbe {
        relocation_saw_no_observers: Arc<AtomicBool>,
        callbacks: Arc<AtomicUsize>,
    }

    impl ThreadAware for ReentrantSpawnProbe {
        fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
            self.relocation_saw_no_observers.store(
                LocalWorker::on_spawn(destination, TaskIdentity::from_location(std::panic::Location::caller())).is_none(),
                Ordering::Release,
            );
        }
    }

    fn test_dispatcher(endpoints: NonEmpty<WorkerEndpoint>, shutdown: Shutdown) -> Arc<DispatcherCore> {
        DispatcherCore::new(endpoints, shutdown, Vec::new(), None)
    }

    #[test]
    fn panic_without_installed_scheduler_defaults_to_shutdown() {
        let (action, panic) = LocalWorker::on_panic(Box::new(42_u32));

        assert_eq!(action, arty_executor::PanicAction::Shutdown);
        assert_eq!(*panic.downcast::<u32>().unwrap(), 42);
    }

    #[test]
    fn no_spawn_hooks_skip_event_creation_without_a_scheduler() {
        let _guard = LOCAL_SPAWN_HOOK.initialize();
        let target = ThreadBuilder::default().build(std::thread::current().id());

        assert!(LocalWorker::on_spawn(&target, TaskIdentity::from_location(std::panic::Location::caller())).is_none());
    }

    #[derive(Clone)]
    struct SpawnSources(Arc<Mutex<Vec<Option<std::thread::ThreadId>>>>);

    impl ThreadAware for SpawnSources {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[test]
    fn dispatcher_uses_worker_local_spawn_hooks_for_remote_submission() {
        let (queued, sources, source_id, scheduler_cleared) = std::thread::spawn(|| {
            let builder = ThreadBuilder::default();
            let destination_builder = builder.clone();
            let source = builder.build(std::thread::current().id());
            let destination = std::thread::spawn(move || destination_builder.build(std::thread::current().id()))
                .join()
                .unwrap();
            let sources = Arc::new(Mutex::new(Vec::new()));
            let hook = registration(HookFn::<TaskSpawn, _>::new(SpawnSources(Arc::clone(&sources)), |event, sources| {
                sources.0.lock().unwrap().push(event.origin().map(Thread::id));
            }));
            let shutdown = Shutdown::default();
            let (source_commands, _source_rx) = crate::runtime::worker::asynchronous::command_channel();
            let (destination_commands, destination_rx) = crate::runtime::worker::asynchronous::command_channel();
            let core = DispatcherCore::new(
                NonEmpty::from_vec(vec![
                    worker_endpoint(
                        source_commands,
                        Waker::noop().clone(),
                        source.clone(),
                        Worker::new(Pool::new(RuntimeName::default(), None, None), shutdown.clone()),
                    ),
                    worker_endpoint(
                        destination_commands,
                        Waker::noop().clone(),
                        destination.clone(),
                        Worker::new(Pool::new(RuntimeName::default(), None, None), shutdown.clone()),
                    ),
                ])
                .unwrap(),
                shutdown,
                vec![hook.clone()],
                None,
            );
            let dispatcher = DispatcherClient::new(Arc::clone(&core));
            let _prime = core.spawn_relocating(
                &destination,
                |_| async {},
                SpawnOptions::new(
                    dispatcher.monitoring(),
                    TaskIdentity::from_location(std::panic::Location::caller()),
                    0,
                    None,
                ),
            );
            sources.lock().unwrap().clear();
            let io = crate::io::Io::new(
                arty_io_host::IoRuntime::new(observed::Sink::noop()),
                dispatcher.clone(),
                source.clone(),
            );
            let (clock, _clock_driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
            let context = Context::new(dispatcher.clone(), clock, io, source.clone());
            let executor = Executor::builder().thread(source.clone()).build();
            LocalWorker::attach(monitoring_worker());
            LocalWorker::install_scheduler(context, executor.tasks(), executor.shutdown(), vec![hook]);

            let _task = core.spawn_relocating(
                &source,
                |_| async {},
                SpawnOptions::new(
                    dispatcher.monitoring(),
                    TaskIdentity::from_location(std::panic::Location::caller()),
                    0,
                    None,
                ),
            );
            let queued = destination_rx.spawn_receiver().try_recv().is_ok();

            LocalWorker::clear_scheduler();
            let (scheduler_cleared, retained_scheduler, retained_spawn_hooks, retained_panic_hooks) =
                LocalWorker::with_current_mut(|current| {
                    let current = current.expect("the test worker remains attached until thread teardown");
                    let cleared = current.scheduler.is_none() && current.spawn_hooks.is_none() && current.panic_hooks.is_none();
                    (
                        cleared,
                        current.scheduler.take(),
                        current.spawn_hooks.take(),
                        current.panic_hooks.take(),
                    )
                });
            if !scheduler_cleared {
                std::mem::forget(retained_scheduler);
                drop(retained_spawn_hooks);
                drop(retained_panic_hooks);
            }
            let observed_sources = sources.lock().unwrap().clone();
            (queued, observed_sources, source.id(), scheduler_cleared)
        })
        .join()
        .unwrap();

        assert!(
            scheduler_cleared,
            "clear_scheduler must release all worker-local registrations before thread teardown"
        );
        assert!(queued);
        assert_eq!(sources, [Some(source_id)]);
    }

    #[test]
    fn spawn_hook_flag_changes_after_first_hook_installation() {
        let (relocation_saw_no_observers, callbacks) = std::thread::spawn(|| {
            let thread = ThreadBuilder::default().build(std::thread::current().id());
            let shutdown = Shutdown::default();
            let (commands, _command_rx) = crate::runtime::worker::asynchronous::command_channel();
            let core = test_dispatcher(
                NonEmpty::new(worker_endpoint(
                    commands,
                    Waker::noop().clone(),
                    thread.clone(),
                    Worker::new(Pool::new(RuntimeName::default(), None, None), shutdown.clone()),
                )),
                shutdown,
            );
            let dispatcher = DispatcherClient::new(core);
            let io = crate::io::Io::new(
                arty_io_host::IoRuntime::new(observed::Sink::noop()),
                dispatcher.clone(),
                thread.clone(),
            );
            let (clock, _clock_driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
            let context = Context::new(dispatcher.clone(), clock, io, thread.clone());
            let executor = Executor::builder().thread(thread.clone()).build();
            let relocation_saw_no_observers = Arc::new(AtomicBool::new(false));
            let callbacks = Arc::new(AtomicUsize::new(0));
            LocalWorker::attach(monitoring_worker());
            LocalWorker::install_scheduler(context, executor.tasks(), executor.shutdown(), Vec::new());

            assert!(LocalWorker::on_spawn(&thread, TaskIdentity::from_location(std::panic::Location::caller())).is_none());
            LocalWorker::install_hook(
                registration(HookFn::<TaskSpawn, _>::new(
                    ReentrantSpawnProbe {
                        relocation_saw_no_observers: Arc::clone(&relocation_saw_no_observers),
                        callbacks: Arc::clone(&callbacks),
                    },
                    |_event, probe| {
                        probe.callbacks.fetch_add(1, Ordering::Relaxed);
                    },
                )),
                None,
            );
            assert!(LocalWorker::on_spawn(&thread, TaskIdentity::from_location(std::panic::Location::caller())).is_none());

            LocalWorker::clear_scheduler();
            (
                relocation_saw_no_observers.load(Ordering::Acquire),
                callbacks.load(Ordering::Relaxed),
            )
        })
        .join()
        .unwrap();

        assert!(relocation_saw_no_observers);
        assert_eq!(callbacks, 1);
    }

    #[test]
    fn relocating_registration_checks_owner_execution_and_shutdown() {
        let observations = std::thread::spawn(|| {
            let identity = || TaskIdentity::from_location(std::panic::Location::caller());
            let thread = ThreadBuilder::default().build(std::thread::current().id());
            let wakes = std::sync::Arc::new(CountingWake::default());
            let shutdown = Shutdown::default();
            let (commands, command_rx) = crate::runtime::worker::asynchronous::command_channel();
            let core = test_dispatcher(
                NonEmpty::new(worker_endpoint(
                    commands,
                    Waker::from(std::sync::Arc::clone(&wakes)),
                    thread.clone(),
                    Worker::new(Pool::new(RuntimeName::default(), None, None), shutdown.clone()),
                )),
                shutdown.clone(),
            );
            let dispatcher = DispatcherClient::new(Arc::clone(&core));
            let io = crate::io::Io::new(
                arty_io_host::IoRuntime::new(observed::Sink::noop()),
                dispatcher.clone(),
                thread.clone(),
            );
            let (clock, _clock_driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
            let context = Context::new(dispatcher.clone(), clock, io, thread.clone());
            let mut executor = Executor::builder().thread(thread.clone()).build();
            LocalWorker::attach(monitoring_worker());
            LocalWorker::install_scheduler(context, executor.tasks(), executor.shutdown(), Vec::new());

            let mut join = std::pin::pin!(core.spawn_relocating(
                &thread,
                |_| async { 42 },
                SpawnOptions::new(dispatcher.monitoring(), identity(), 0, None),
            ));
            let registered_locally = command_rx.spawn_receiver().try_recv().is_err() && executor.has_tasks();
            let idle_worker_woken = wakes.0.load(Ordering::Relaxed) == 1;
            _ = executor.execute_cycle();
            let local_result = matches!(
                join.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())),
                Poll::Ready(Ok(42))
            );

            let nested_core = Arc::clone(&core);
            let nested_dispatcher = dispatcher.clone();
            let source = thread.clone();
            let nested_wakes = std::sync::Arc::clone(&wakes);
            let mut parent = std::pin::pin!(core.spawn_relocating(
                &thread,
                move |_| async move {
                    let before = nested_wakes.0.load(Ordering::Relaxed);
                    let child = nested_core.spawn_relocating(
                        &source,
                        |_| async { 7 },
                        SpawnOptions::new(nested_dispatcher.monitoring(), identity(), 0, None),
                    );
                    let executing_worker_not_woken = nested_wakes.0.load(Ordering::Relaxed) == before;
                    (child.await.unwrap(), executing_worker_not_woken)
                },
                SpawnOptions::new(dispatcher.monitoring(), identity(), 0, None)
            ));
            _ = executor.execute_cycle();
            _ = executor.execute_cycle();
            _ = executor.execute_cycle();
            let nested_result = matches!(
                parent.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())),
                Poll::Ready(Ok((7, true)))
            );
            let nested_registered_locally = command_rx.spawn_receiver().try_recv().is_err();
            let other_owner_queued = other_owner_receives_queued_registration();
            let other_owner_not_registered = !executor.has_tasks();
            shutdown.begin(ShutdownCause::default());
            let before = wakes.0.load(Ordering::Relaxed);
            let mut rejected = std::pin::pin!(core.spawn_relocating(
                &thread,
                |_| async { 11 },
                SpawnOptions::new(dispatcher.monitoring(), identity(), 0, None),
            ));
            let rejected_after_shutdown = matches!(
                rejected.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())),
                Poll::Ready(Err(error)) if error.is_shutdown()
            );
            let rejection_not_queued = command_rx.spawn_receiver().try_recv().is_err();
            let rejection_not_woken = wakes.0.load(Ordering::Relaxed) == before;
            LocalWorker::clear_scheduler();
            let (scheduler_cleared, retained_scheduler, retained_hooks) = LocalWorker::with_current_mut(|current| {
                let current = current.expect("the test worker remains attached until thread teardown");
                let cleared = current.scheduler.is_none() && current.panic_hooks.is_none();
                (cleared, current.scheduler.take(), current.panic_hooks.take())
            });
            drop(retained_scheduler);
            drop(retained_hooks);
            [
                ("same-owner registration stays local", registered_locally),
                ("idle worker is woken", idle_worker_woken),
                ("local result completes", local_result),
                ("nested result completes without waking the executing worker", nested_result),
                ("nested registration stays local", nested_registered_locally),
                ("other owner receives a queued command", other_owner_queued),
                ("other owner's task is not registered locally", other_owner_not_registered),
                ("shutdown rejects registration", rejected_after_shutdown),
                ("shutdown rejection sends no command", rejection_not_queued),
                ("shutdown rejection does not wake the worker", rejection_not_woken),
                ("scheduler cleanup releases worker-local state", scheduler_cleared),
            ]
        })
        .join()
        .unwrap();
        for (description, observed) in observations {
            assert!(observed, "{description}");
        }
    }

    fn other_owner_receives_queued_registration() -> bool {
        let thread = ThreadBuilder::default().build(std::thread::current().id());
        let (commands, receiver) = crate::runtime::worker::asynchronous::command_channel();
        let core = test_dispatcher(
            NonEmpty::new(worker_endpoint(
                commands,
                Waker::noop().clone(),
                thread.clone(),
                Worker::new(Pool::new(RuntimeName::default(), None, None), Shutdown::default()),
            )),
            Shutdown::default(),
        );
        let dispatcher = DispatcherClient::new(Arc::clone(&core));
        let _remote = core.spawn_relocating(
            &thread,
            |_| async { 9 },
            SpawnOptions::new(
                dispatcher.monitoring(),
                TaskIdentity::from_location(std::panic::Location::caller()),
                0,
                None,
            ),
        );
        matches!(receiver.spawn_receiver().try_recv(), Ok(SpawnCommand { .. }))
    }

    thread_local! {
        static EXTERNAL_OWNER: ExternalOwner = const { ExternalOwner };
    }

    struct ExternalOwner;

    impl Drop for ExternalOwner {
        fn drop(&mut self) {
            assert!(LocalWorker::try_with_driver_host(|_| ()).is_none());
        }
    }

    #[test]
    fn external_lookup_does_not_access_destroyed_worker_slot() {
        std::thread::spawn(|| {
            EXTERNAL_OWNER.with(|_| {});
            LocalWorker::with_scheduler(|_| ());
        })
        .join()
        .unwrap();
    }
}
