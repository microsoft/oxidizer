// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::Debug;
use std::sync::Arc as StdArc;
use std::task::{Wake, Waker};
use std::thread::ThreadId;

use arty_executor::ShutdownCause;
use performables::arc::Arc;
use thread_aware::ThreadAware;

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::dispatch::{DispatcherCore, WorkerIndex};
use crate::runtime::shutdown::Shutdown;
use crate::runtime::{Error, Workers};
use crate::task::Builtins;
use crate::task::join::JoinHandle;

/// Cheap shared access to one runtime's routing and shutdown state.
#[derive(Clone)]
pub(crate) struct DispatcherClient {
    core: Arc<DispatcherCore>,
}

struct PanicShutdownWake(DispatcherClient);

impl Wake for PanicShutdownWake {
    fn wake(self: StdArc<Self>) {
        self.0.record_panic();
    }

    fn wake_by_ref(self: &StdArc<Self>) {
        self.0.record_panic();
    }
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
            blocking_worker: BlockingWorker::new(
                crate::runtime::blocking_worker::BlockingPool::new(None),
                observed::Sink::noop(),
                shutdown.clone(),
            ),
        };
        Self::new(Arc::new(DispatcherCore::new(
            crate::runtime::thread::waiter::ThreadWaiter::new(Vec::new()),
            nonempty::NonEmpty::new(endpoint),
            observed::Sink::noop(),
            shutdown,
        )))
    }

    pub(crate) fn worker_index(&self, thread_id: ThreadId) -> Option<WorkerIndex> {
        self.core.worker_index(thread_id)
    }

    pub(crate) fn workers(&self) -> &Workers {
        self.core.workers()
    }

    pub(crate) fn blocking_worker(&self, worker_index: WorkerIndex) -> Arc<BlockingWorker> {
        self.core.blocking_worker(worker_index)
    }

    pub(crate) fn wake_worker(&self, worker_index: WorkerIndex) {
        self.core.wake_worker(worker_index);
    }

    pub(crate) fn next_blocking_worker(&self) -> Arc<BlockingWorker> {
        self.core.next_blocking_worker()
    }

    pub(crate) fn owns(&self, thread: &thread_aware::Thread) -> bool {
        self.core.owns(thread)
    }

    pub(crate) fn shutdown(&self) -> &Shutdown {
        self.core.shutdown()
    }

    #[cfg_attr(test, mutants::skip)] // Removing this wait guard deadlocks runtime-owner teardown.
    pub(crate) fn is_current_blocking_thread(&self) -> bool {
        self.core.is_current_blocking_thread()
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

    pub(crate) fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.core.spawn(future_factory)
    }

    pub(crate) fn spawn_relocating<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        self.core.spawn_relocating(future_factory)
    }

    pub(crate) fn spawn_at_worker_relocating<FF, F, R>(&self, worker_index: WorkerIndex, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        self.core.spawn_at_worker_relocating(worker_index, future_factory)
    }

    pub(crate) fn stop_with_error(&self, error: Error) -> ShutdownCause {
        self.core.stop_with_error(error)
    }

    pub(crate) fn record_panic(&self) {
        self.core.record_panic();
    }

    pub(crate) fn panic_result_hook(&self) -> Waker {
        Waker::from(StdArc::new(PanicShutdownWake(self.clone())))
    }

    pub(crate) fn access_io(&self, worker_index: WorkerIndex, access: Box<dyn FnOnce(&Builtins) + Send>) -> Result<(), Error> {
        self.core.access_io(worker_index, access)
    }

    #[cfg_attr(test, mutants::skip)] // Will cause tests to hang due to runtime never stopping.
    pub(crate) fn stop(&self) -> ShutdownCause {
        self.core.stop()
    }

    // Impractical to test real waiting at this API layer. We test the real waiter implementation
    // but not the API layers that simply call the waiter, as it is hard to prove the wait failed.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn wait(&self) -> Result<(), Error> {
        self.core.join()
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
    use crate::runtime::blocking_worker::BlockingPool;
    use crate::runtime::dispatch::{WorkerEndpoint, test_threads};
    use crate::runtime::thread::waiter::ThreadWaiter;
    use crate::task::scheduler::Scheduler;

    #[test]
    fn clients_observe_the_shared_shutdown_state() {
        let (command_tx, _commands) = channel::unbounded();
        let worker = WorkerEndpoint {
            command_tx,
            waker: Waker::noop().clone(),
            thread: test_threads(1).pop().unwrap(),
            blocking_worker: BlockingWorker::new(BlockingPool::new(None), Sink::noop(), Shutdown::default()),
        };
        let client = DispatcherClient::new(Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![]),
            nonempty::NonEmpty::new(worker),
            Sink::noop(),
            Shutdown::default(),
        )));
        let clone = client.clone();
        assert_eq!(client.shutdown().state(), crate::runtime::ShutdownState::Running);
        assert_eq!(clone.shutdown().state(), crate::runtime::ShutdownState::Running);

        client.stop();
        assert_eq!(client.shutdown().state(), crate::runtime::ShutdownState::Started);
        assert_eq!(clone.shutdown().state(), crate::runtime::ShutdownState::Started);
    }

    #[test]
    fn relocate_updates_task_placement() {
        // Create a dispatcher with 2 workers so we can verify which one receives tasks.
        let (worker0_tx, worker0_rx) = channel::unbounded();
        let (worker1_tx, worker1_rx) = channel::unbounded();
        let wfs = ThreadWaiter::new(vec![]);
        let threads = test_threads(2);
        let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop(), Shutdown::default());

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
            Shutdown::default(),
        ));

        let mut scheduler = Scheduler::new(DispatcherClient::new(dispatcher), threads[0].clone());

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

        let source_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop(), Shutdown::default());
        let destination_shutdown = Shutdown::default();
        let destination_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop(), destination_shutdown.clone());
        destination_shutdown.begin(ShutdownCause::default());

        let dispatcher = Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![]),
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
            Sink::noop(),
            Shutdown::default(),
        ));

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
            blocking_worker: BlockingWorker::new(BlockingPool::new(None), Sink::noop(), Shutdown::default()),
        };
        let dispatcher = DispatcherClient::new(Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![]),
            nonempty::NonEmpty::new(worker),
            Sink::noop(),
            Shutdown::default(),
        )));
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
