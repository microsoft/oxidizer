// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;

use arty_executor::{Shutdown as ExecutorShutdown, ShutdownCause, TaskPlacement, TaskSet};
use thread_aware::{Thread, ThreadAware};

use crate::io::RuntimeDriverHost;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::hook::{ErasedHookFn, HookRegistration, Panic, PanicAction};
use crate::runtime::worker::{self, WorkerMarker};
use crate::task::Context;
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::DeferredTaskFactory;

thread_local! {
    static CURRENT: RefCell<Option<LocalWorker>> = const { RefCell::new(None) };
    static PANIC_HOOKS: RefCell<Option<Vec<ErasedHookFn<Panic>>>> = const { RefCell::new(None) };
}

/// Asynchronous worker state that remains attached throughout thread teardown.
#[derive(Debug)]
pub(crate) struct LocalWorker {
    scheduler: Option<SchedulerContext>,
    driver_host: Option<RuntimeDriverHost>,
}

impl LocalWorker {
    fn new() -> Self {
        Self {
            scheduler: None,
            driver_host: None,
        }
    }

    pub(crate) fn attach() {
        worker::WorkerMarker::attach();
        CURRENT.with_borrow_mut(|current| {
            assert!(current.is_none(), "a task local worker is already attached to this thread");
            *current = Some(Self::new());
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
        };
        PANIC_HOOKS.with_borrow_mut(|current| {
            assert!(
                current.replace(Vec::new()).is_none(),
                "panic hooks are already installed on this task worker"
            );
        });
        Self::with_current_mut(|current| {
            let current = current.expect("task worker startup attaches its local worker before installing the scheduler");
            assert!(current.scheduler.is_none(), "a scheduler is already installed on this task worker");
            current.scheduler = Some(scheduler);
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

    pub(crate) fn on_panic(panic: arty_executor::TaskPanicPayload) -> (PanicAction, arty_executor::TaskPanicPayload) {
        if !Self::with_scheduler(|current| current.is_some()) {
            return Panic::new(panic).into_parts();
        }
        let mut event = Panic::new(panic);
        PANIC_HOOKS.with_borrow_mut(|hooks| {
            for hook in hooks.as_mut().expect("a running worker retains its installed panic hooks") {
                hook.execute(&mut event);
            }
        });
        event.into_parts()
    }

    pub(crate) fn install_hook(hook: HookRegistration, source: Option<&Thread>) {
        let destination = Self::with_scheduler(|current| {
            let current = current.expect("a running worker retains its scheduler while processing hook registration");
            current.thread.clone()
        });
        let mut hook = hook;
        hook.relocate(source, &destination);
        PANIC_HOOKS.with_borrow_mut(|hooks| {
            let hooks = hooks.as_mut().expect("a running worker retains its installed panic hooks");
            match hook {
                HookRegistration::Panic(hook) => hooks.push(hook),
            }
        });
    }

    #[cfg_attr(test, mutants::skip)] // Retaining direct registration during executor teardown can strand shutdown.
    pub(super) fn clear_scheduler() {
        let scheduler = Self::with_current_mut(|current| current.and_then(|current| current.scheduler.take()));
        drop(scheduler);
        let hooks = PANIC_HOOKS.with_borrow_mut(Option::take);
        drop(hooks);
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
        if !WorkerMarker::is_current() {
            return None;
        }
        Self::with_current(|current| current.and_then(|current| current.driver_host.as_ref().map(f)))
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

/// Task worker state available while direct task registration is safe.
#[derive(Debug)]
pub(crate) struct SchedulerContext {
    thread: Thread,
    context: Context,
    tasks: TaskSet,
    executor_shutdown: ExecutorShutdown,
    results: JoinHandlePool,
}

impl SchedulerContext {
    pub(crate) fn thread(&self) -> &Thread {
        &self.thread
    }

    pub(crate) fn tasks(&self) -> &TaskSet {
        &self.tasks
    }

    pub(crate) fn spawn<FF, F, R>(&self, future_factory: FF, placement: TaskPlacement) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let (handle, result_tx) = self.results.handle();
        let inner = DeferredTaskFactory::new(future_factory, self.context.clone());
        self.tasks.task(inner).placement(placement).register(result_tx);
        handle
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Poll, Wake, Waker};

    use arty_executor::Executor;
    use nonempty::NonEmpty;
    use performables::arc::Arc;
    use thread_aware::ThreadBuilder;
    use tick::runtime::InactiveClock;

    use super::*;
    use crate::runtime::config::RuntimeName;
    use crate::runtime::dispatch::DispatcherCore;
    use crate::runtime::shutdown::Shutdown;
    use crate::runtime::testing::worker_endpoint;
    use crate::runtime::worker::asynchronous::SpawnCommand;
    use crate::runtime::worker::blocking::{Pool, Worker};

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

    #[test]
    fn panic_without_installed_scheduler_defaults_to_shutdown() {
        let (action, panic) = LocalWorker::on_panic(Box::new(42_u32));

        assert_eq!(action, PanicAction::Shutdown);
        assert_eq!(*panic.downcast::<u32>().unwrap(), 42);
    }

    #[test]
    fn relocating_registration_checks_owner_execution_and_shutdown() {
        let observations = std::thread::spawn(|| {
            let thread = ThreadBuilder::default().build(std::thread::current().id());
            let wakes = std::sync::Arc::new(CountingWake::default());
            let shutdown = Shutdown::default();
            let (commands, command_rx) = crate::runtime::worker::asynchronous::command_channel();
            let core = DispatcherCore::new(
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
            let context = Context::new(dispatcher, clock, io, thread.clone());
            let mut executor = Executor::builder().thread(thread.clone()).build();
            LocalWorker::attach();
            LocalWorker::install_scheduler(context, executor.tasks(), executor.shutdown(), Vec::new());

            let mut join = std::pin::pin!(core.spawn_relocating(&thread, |_| async { 42 }));
            let registered_locally = command_rx.spawn_receiver().try_recv().is_err() && executor.has_tasks();
            let idle_worker_woken = wakes.0.load(Ordering::Relaxed) == 1;
            _ = executor.execute_cycle();
            let local_result = matches!(
                join.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())),
                Poll::Ready(Ok(42))
            );

            let nested_core = Arc::clone(&core);
            let source = thread.clone();
            let nested_wakes = std::sync::Arc::clone(&wakes);
            let mut parent = std::pin::pin!(core.spawn_relocating(&thread, move |_| async move {
                let before = nested_wakes.0.load(Ordering::Relaxed);
                let child = nested_core.spawn_relocating(&source, |_| async { 7 });
                let executing_worker_not_woken = nested_wakes.0.load(Ordering::Relaxed) == before;
                (child.await.unwrap(), executing_worker_not_woken)
            }));
            _ = executor.execute_cycle();
            _ = executor.execute_cycle();
            _ = executor.execute_cycle();
            let nested_result = matches!(
                parent.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())),
                Poll::Ready(Ok((7, true)))
            );
            let nested_registered_locally = command_rx.spawn_receiver().try_recv().is_err();

            let other_thread = ThreadBuilder::default().build(std::thread::current().id());
            let (other_tx, other_rx) = crate::runtime::worker::asynchronous::command_channel();
            let other_core = DispatcherCore::new(
                NonEmpty::new(worker_endpoint(
                    other_tx,
                    Waker::noop().clone(),
                    other_thread.clone(),
                    Worker::new(Pool::new(RuntimeName::default(), None, None), Shutdown::default()),
                )),
                Shutdown::default(),
            );
            let _remote = other_core.spawn_relocating(&other_thread, |_| async { 9 });
            let other_owner_queued = matches!(other_rx.spawn_receiver().try_recv(), Ok(SpawnCommand { .. }));
            let other_owner_not_registered = !executor.has_tasks();

            shutdown.begin(ShutdownCause::default());
            let before = wakes.0.load(Ordering::Relaxed);
            let mut rejected = std::pin::pin!(core.spawn_relocating(&thread, |_| async { 11 }));
            let rejected_after_shutdown = matches!(
                rejected.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())),
                Poll::Ready(Err(error)) if error.is_shutdown()
            );
            let rejection_not_queued = command_rx.spawn_receiver().try_recv().is_err();
            let rejection_not_woken = wakes.0.load(Ordering::Relaxed) == before;
            LocalWorker::clear_scheduler();
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
            ]
        })
        .join()
        .unwrap();
        for (description, observed) in observations {
            assert!(observed, "{description}");
        }
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
