// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime submission and control capabilities.

use std::any::type_name;
use std::fmt::Debug;
use std::thread::ThreadId;

use arty_executor::ShutdownCause;
use performables::arc::Arc;
use thread_aware::ThreadAware;

use crate::runtime::dispatch::{DispatcherCore, WorkerIndex};
use crate::runtime::shutdown::Shutdown;
use crate::runtime::worker::blocking::Worker;
use crate::runtime::{Error, Workers};
use crate::task::Context;
use crate::task::handle::JoinHandle;

/// Cheap shared access to one runtime's routing and shutdown state.
#[derive(Clone)]
pub(crate) struct DispatcherClient {
    core: Arc<DispatcherCore>,
}

impl DispatcherClient {
    pub(crate) const fn new(core: Arc<DispatcherCore>) -> Self {
        Self { core }
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn for_test(thread: &thread_aware::Thread) -> Self {
        let (command_tx, _command_rx) = performables::sync::channel::unbounded();
        let shutdown = Shutdown::default();
        let endpoint = crate::runtime::dispatch::WorkerEndpoint {
            command_tx,
            waker: std::task::Waker::noop().clone(),
            thread: thread.clone(),
            blocking_worker: Worker::new(
                crate::runtime::worker::blocking::Pool::new(None),
                shutdown.clone(),
                crate::runtime::worker::blocking::PanicPolicy::Ignore,
            ),
        };
        Self::new(DispatcherCore::new(nonempty::NonEmpty::new(endpoint), shutdown))
    }

    pub(crate) fn worker_index(&self, thread_id: ThreadId) -> Option<WorkerIndex> {
        self.core.worker_index(thread_id)
    }

    pub(crate) fn workers(&self) -> &Workers {
        self.core.workers()
    }

    pub(crate) fn blocking_worker(&self, worker_index: WorkerIndex) -> Arc<Worker> {
        self.core.blocking_worker(worker_index)
    }

    pub(crate) fn wake_worker(&self, worker_index: WorkerIndex) {
        self.core.wake_worker(worker_index);
    }

    pub(crate) fn next_blocking_worker(&self) -> Arc<Worker> {
        self.core.next_blocking_worker()
    }

    pub(crate) fn owns(&self, thread: &thread_aware::Thread) -> bool {
        self.core.owns(thread)
    }

    pub(crate) fn shutdown(&self) -> &Shutdown {
        self.core.shutdown()
    }

    #[cfg_attr(test, mutants::skip)] // Removing this wait guard deadlocks runtime-owner teardown.
    /// Submits to a previously resolved worker without discarding destination affinity.
    pub(crate) fn spawn_on_worker<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core.spawn_on_worker(worker_index, future_factory)
    }

    pub(crate) fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core.spawn(future_factory)
    }

    pub(crate) fn spawn_relocating<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        self.core.spawn_relocating(future_factory)
    }

    pub(crate) fn spawn_at_worker_relocating<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        self.core.spawn_at_worker_relocating(worker_index, future_factory)
    }

    pub(crate) fn access_io(&self, worker_index: WorkerIndex, access: Box<dyn FnOnce(&Context) + Send>) -> Result<(), Error> {
        self.core.access_io(worker_index, access)
    }

    #[cfg_attr(test, mutants::skip)] // Will cause tests to hang due to runtime never stopping.
    pub(crate) fn stop(&self, cause: impl Into<ShutdownCause>) -> ShutdownCause {
        self.core.stop_inner(cause.into())
    }
}

impl Debug for DispatcherClient {
    #[cfg_attr(coverage_nightly, coverage(off))] // Opaque diagnostic formatting only.
    #[cfg_attr(test, mutants::skip)] // Debug formatting not tested
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::task::Waker;

    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
    use observed::Sink;
    use performables::sync::channel;
    use thread_aware::Thread;

    use super::*;
    use crate::runtime::dispatch::{WorkerEndpoint, test_threads};
    use crate::runtime::worker::blocking::{PanicPolicy, Pool};
    use crate::task::scheduler::Scheduler;

    #[test]
    fn gamma_survivor_private_io_closed_response_returns_error_and_preserves_thread_debug() {
        use arty_io_host::IoRuntime;

        use crate::io::Io;
        use crate::runtime::worker::asynchronous::Command;

        struct Options(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl Clone for Options {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Self(std::sync::Arc::clone(&self.0))
            }
        }
        #[derive(Clone)]
        struct ProbeContext;
        impl ThreadAware for ProbeContext {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
        impl IoContext for ProbeContext {
            type ContextOptions = Options;
            type Driver = TestDriver;

            fn context(_options: Self::ContextOptions) -> Self {
                Self
            }

            fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
                Ok(TestDriver)
            }
        }

        let thread = test_threads(1).remove(0);
        let (tx, rx) = channel::unbounded();
        let endpoint = WorkerEndpoint {
            command_tx: tx,
            waker: Waker::noop().clone(),
            thread: thread.clone(),
            blocking_worker: Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore),
        };
        let shutdown = Shutdown::default();
        let dispatcher = DispatcherClient::new(DispatcherCore::new(nonempty::NonEmpty::new(endpoint), shutdown.clone()));
        let io = Io::new(IoRuntime::new(Sink::noop()), dispatcher, thread.clone());
        assert_eq!(format!("{io:?}"), format!("{} {{ thread: {thread:?}, .. }}", type_name::<Io>()));
        let worker_shutdown = shutdown;
        let worker = std::thread::spawn(move || {
            let Command::AccessIo { access } = rx.recv().unwrap() else {
                panic!("the external context request must enqueue an I/O command");
            };
            worker_shutdown.begin(ShutdownCause::default());
            drop(access);
        });
        let result = io.try_context::<TestIo>(&());
        assert_eq!(result.err().unwrap().to_string(), Error::runtime_shutting_down().to_string());
        worker.join().unwrap();
        let clones = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        assert!(io.try_context::<ProbeContext>(&Options(std::sync::Arc::clone(&clones))).is_err());
        assert_eq!(clones.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn clients_observe_the_shared_shutdown_state() {
        let (command_tx, _commands) = channel::unbounded();
        let worker = WorkerEndpoint {
            command_tx,
            waker: Waker::noop().clone(),
            thread: test_threads(1).pop().unwrap(),
            blocking_worker: Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore),
        };
        let client = DispatcherClient::new(DispatcherCore::new(nonempty::NonEmpty::new(worker), Shutdown::default()));
        let clone = client.clone();
        assert_eq!(client.shutdown().state(), crate::runtime::ShutdownState::Running);
        assert_eq!(clone.shutdown().state(), crate::runtime::ShutdownState::Running);
        client.stop(ShutdownCause::default());
        assert_eq!(client.shutdown().state(), crate::runtime::ShutdownState::Started);
        assert_eq!(clone.shutdown().state(), crate::runtime::ShutdownState::Started);
    }

    #[test]
    fn gamma_survivor_private_context_and_scheduler_relocation_preserve_clone_and_worker_bindings() {
        use crate::io::Io;
        use crate::runtime::worker::asynchronous::Command;

        let threads = test_threads(2);
        let (first_tx, first_rx) = channel::unbounded();
        let (second_tx, second_rx) = channel::unbounded();
        let endpoints = nonempty::NonEmpty::from_vec(vec![
            WorkerEndpoint {
                command_tx: first_tx,
                waker: Waker::noop().clone(),
                thread: threads[0].clone(),
                blocking_worker: Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore),
            },
            WorkerEndpoint {
                command_tx: second_tx,
                waker: Waker::noop().clone(),
                thread: threads[1].clone(),
                blocking_worker: Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore),
            },
        ])
        .unwrap();
        let dispatcher = DispatcherClient::new(DispatcherCore::new(endpoints, Shutdown::default()));
        let io = Io::new(arty_io_host::IoRuntime::new(Sink::noop()), dispatcher.clone(), threads[0].clone());
        let mut context = Context::new(dispatcher, tick::Clock::new_frozen(), io, threads[0].clone());
        let source = context.clone();
        assert!(std::ptr::eq(context.thread(), source.thread()));
        context.relocate(None, &threads[0]);
        assert!(std::ptr::eq(context.thread(), source.thread()));
        context.relocate(Some(&threads[0]), &threads[1]);
        assert_eq!(context.thread(), &threads[1]);
        assert_eq!(source.thread(), &threads[0]);
        assert!(!std::ptr::eq(context.thread(), source.thread()));
        assert!(format!("{:?}", context.io()).contains(&format!("thread: {:?}", threads[1])));

        let foreign = test_threads(1).remove(0);
        context.relocate(Some(&threads[1]), &foreign);
        assert_eq!(context.thread(), &threads[1]);
        drop(context.scheduler().spawn(async |_| 17));
        assert!(matches!(second_rx.try_recv().unwrap(), Command::EnqueueTask { .. }));
        first_rx.try_recv().unwrap_err();
        drop(source.scheduler().spawn(async |_| 19));
        assert!(matches!(first_rx.try_recv().unwrap(), Command::EnqueueTask { .. }));
        second_rx.try_recv().unwrap_err();

        let mut scheduler = source.scheduler().clone();
        scheduler.relocate(None, &threads[1]);
        scheduler.relocate(Some(&threads[1]), &foreign);
        drop(scheduler.spawn(async |_| 23));
        assert!(matches!(second_rx.try_recv().unwrap(), Command::EnqueueTask { .. }));
        first_rx.try_recv().unwrap_err();
    }

    #[test]
    fn relocate_updates_task_placement() {
        // Create two workers and observe which one receives each task.
        let (worker0_tx, worker0_rx) = channel::unbounded();
        let (worker1_tx, worker1_rx) = channel::unbounded();
        let threads = test_threads(2);
        let blocking_worker = Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore);

        let dispatcher = DispatcherCore::new(
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
            Shutdown::default(),
        );

        let mut scheduler = Scheduler::new(DispatcherClient::new(dispatcher), threads[0].clone());

        // Relocate to worker 1.
        scheduler.relocate(Some(&threads[0]), &threads[1]);

        // Spawn a task on worker 1, not worker 0.
        scheduler.spawn(async |_| {});

        // Worker 0 receives no task.
        assert!(
            worker0_rx.try_recv().is_err(),
            "task must not dispatch to the original worker after relocation"
        );
        // Worker 1 receives the task.
        assert!(worker1_rx.try_recv().is_ok(), "task must dispatch to the relocated worker");
    }

    #[test]
    fn relocate_switches_blocking_worker() {
        let threads = test_threads(2);

        let source_worker = Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore);
        let destination_shutdown = Shutdown::default();
        let destination_worker = Worker::new(Pool::new(None), destination_shutdown.clone(), PanicPolicy::Ignore);
        destination_shutdown.begin(ShutdownCause::default());

        let dispatcher = DispatcherCore::new(
            nonempty::NonEmpty::from_vec(vec![
                WorkerEndpoint {
                    command_tx: channel::unbounded().0,
                    waker: Waker::noop().clone(),
                    thread: threads[0].clone(),
                    blocking_worker: Arc::clone(&source_worker),
                },
                WorkerEndpoint {
                    command_tx: channel::unbounded().0,
                    waker: Waker::noop().clone(),
                    thread: threads[1].clone(),
                    blocking_worker: Arc::clone(&destination_worker),
                },
            ])
            .unwrap(),
            Shutdown::default(),
        );

        let mut scheduler = Scheduler::new(DispatcherClient::new(dispatcher), threads[0].clone());

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

    #[derive(Clone)]
    struct TestIo;

    impl ThreadAware for TestIo {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for TestIo {
        type ContextOptions = ();
        type Driver = TestDriver;

        fn context((): Self::ContextOptions) -> Self {
            Self
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            Ok(TestDriver)
        }
    }

    struct TestDriver;

    impl Driver for TestDriver {
        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    fn io(registered: bool) -> crate::io::Io {
        let threads = test_threads(2);
        let (command_tx, command_rx) = channel::unbounded();
        drop(command_rx);
        let worker = WorkerEndpoint {
            command_tx,
            waker: Waker::noop().clone(),
            thread: threads[0].clone(),
            blocking_worker: Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore),
        };
        let dispatcher = DispatcherClient::new(DispatcherCore::new(nonempty::NonEmpty::new(worker), Shutdown::default()));
        let thread = if registered { threads[0].clone() } else { threads[1].clone() };
        crate::io::Io::new(arty_io_host::IoRuntime::new(Sink::noop()), dispatcher, thread)
    }

    #[test]
    fn context_access_rejects_an_unregistered_worker() {
        assert!(io(false).try_context::<TestIo>(&()).is_err());
    }

    #[test]
    fn context_access_rejects_a_closed_worker_channel() {
        assert!(io(true).try_context::<TestIo>(&()).is_err());
    }
}
