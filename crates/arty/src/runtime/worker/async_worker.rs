// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use arty_executor::{CycleOutcome, Executor, TaskSet};
use observed::Sink;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::Thread;
use tick::runtime::{ClockDriver, InactiveClock};

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::worker::protocol::AsyncWorkerCommand;
use crate::runtime::worker::signal::WorkerSignal;
use crate::task::{Builtins, Scheduler};

/// If we have nothing to do, we wait for something to happen for this long before executing
/// another executor cycle (just in case something has showed up that we did not notice).
const SUSPEND_SLEEP_DURATION: Duration = Duration::from_millis(1);

const COMMANDS_PER_CYCLE: usize = 256;

/// The async worker has exclusive use of a runtime thread, one per processor selected by
/// the configured processor-count policy. The worker executes async tasks that are
/// woken by local or remote futures, incoming commands, and timers.
///
/// The type represents the internal state of the worker. This state is connected to other objects
/// through messaging channels that allow components of the runtime to deliver work and signals.
///
/// # Ownership
///
/// This type is exclusively owned by the thread entry point.
///
/// # Lifecycle
///
/// The worker lifecycle consists of multiple stages:
///
/// 1. Startup - bootstrap creates the thread and worker from the configuration supplied by
///    [`RuntimeBuilder`][crate::runtime::RuntimeBuilder]. Bootstrap and the worker coordinate
///    initialization until the worker is ready to start operating.
/// 2. Running - the worker is processing tasks, advancing timers, and waiting for commands.
/// 3. Shutting down - the worker has received a command to shut down. It rejects queued and new
///    submissions, stops blocking work, drops the executor to cancel async tasks, and runs final
///    timer callbacks.
/// 4. Dead - the worker has completed its shutdown process and the thread has terminated.
///
/// Once in the Running state, workers keep running until they receive a stop command (i.e. until
/// something calls [`request_stop()`][crate::runtime::RuntimeOperations::request_stop]
/// or stops or drops the runtime owner).
///
/// # Thread safety
///
/// This type is single-threaded - it only exists on the designated worker thread.
#[derive(Debug)]
pub(crate) struct AsyncWorker {
    // Exclusively used by top level worker logic, never via reentrant task logic.
    // We drop this when we receive the shutdown signal, to ensure that any tasks that
    // happen to be enqueued after shutdown starts are discarded when an attempt is
    // made to send them.
    command_rx: Option<channel::Receiver<AsyncWorkerCommand>>,

    // Dropped when shutdown starts so no more tasks can be enqueued and worker-local
    // services can release resources needed by shutdown.
    builtins: Option<Builtins>,

    // Published before shutdown commands are queued, so command backlogs can be rejected without
    // invoking factories that preceded the shutdown command. Acquire loads only need to observe
    // this published flag; command-channel synchronization owns command contents and ordering.
    shutdown_signal: Arc<AtomicBool>,

    // Becomes None during shutdown when the executor is dropped to cancel and release its tasks.
    executor: Option<Executor>,

    // Connected to the `executor`. Submissions after shutdown resolve to an executor-shutdown
    // JoinError rather than registering a task.
    tasks: TaskSet,

    // We keep a reference to the blocking worker so that we can send it a shutdown signal.
    blocking_worker: Arc<BlockingWorker>,

    // This drives moves the timers registered with the clock forward.
    clock_driver: ClockDriver,

    signal: Arc<WorkerSignal>,
    _single_threaded: PhantomData<*const ()>,
}

impl AsyncWorker {
    pub(crate) fn new(
        command_rx: channel::Receiver<AsyncWorkerCommand>,
        dispatcher: DispatcherClient,
        thread: Thread,
        sink: Sink,
        blocking_worker: Arc<BlockingWorker>,
        clock: InactiveClock,
        signal: Arc<WorkerSignal>,
    ) -> Self {
        let (clock, clock_driver) = clock.activate();
        let shutdown_signal = dispatcher.shutdown_signal();

        let executor = Executor::builder()
            .owner_waker(WorkerSignal::waker(&signal))
            .sink(blocking_worker.sink().clone())
            .thread(thread.clone())
            .build();
        let tasks = executor.tasks();

        let builtins = Builtins::new(dispatcher, clock, thread, sink);
        Scheduler::register_current(builtins.clone(), tasks.clone());

        Self {
            command_rx: Some(command_rx),
            builtins: Some(builtins),
            shutdown_signal,
            executor: Some(executor),
            tasks,
            clock_driver,
            blocking_worker,
            signal,
            _single_threaded: PhantomData,
        }
    }

    /// Worker thread entry point, invoked by bootstrap after startup coordination completes.
    /// The configuration-facing `RuntimeBuilder` delegates construction to bootstrap.
    /// After this method returns, the thread will end.
    pub(crate) fn run(mut self) {
        self.execute_phase();
    }

    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn execute_phase(&mut self) {
        while matches!(self.execute_step(), ControlFlow::Continue(())) {}
    }

    fn execute_step(&mut self) -> ControlFlow<()> {
        let batch_exhausted = self.process_commands();

        if self.shutdown_signal.load(Ordering::Acquire) {
            self.begin_shutdown();
            _ = self.clock_driver.advance_timers(Instant::now());
            return ControlFlow::Break(());
        }

        let Some(executor) = self.executor.as_mut() else {
            // Executor drop cancels tasks before final timer callbacks can observe their state.
            _ = self.clock_driver.advance_timers(Instant::now());
            return ControlFlow::Break(());
        };

        let cycle_outcome = executor.execute_cycle();

        if matches!(cycle_outcome, CycleOutcome::Shutdown) {
            // An alternate low-level owner initiated executor shutdown. Complete the surrounding
            // worker shutdown before final timer callbacks run.
            self.begin_shutdown();
            _ = self.clock_driver.advance_timers(Instant::now());
            return ControlFlow::Break(());
        }

        // Advances the timers registered with the clock.
        _ = self.clock_driver.advance_timers(Instant::now());

        if matches!(cycle_outcome, CycleOutcome::Suspend) && !batch_exhausted {
            // Retain the donor's timer cadence, including externally controlled clocks.
            // A notification arriving before this wait is retained by WorkerSignal.
            self.signal.wait(SUSPEND_SLEEP_DURATION);
        }

        ControlFlow::Continue(())
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing requires a fine level of control over what is in the channel, which is too bothersome just for mutation testing.
    fn process_commands(&mut self) -> bool {
        let Some(builtins) = self.builtins.as_ref() else {
            debug_assert!(self.command_rx.is_none());

            // We are shutting down and have dropped worker-local services.
            // The command channel is also closed, so no more commands can be received.
            return false;
        };

        let command_rx = self.command_rx.as_ref().expect("builtins and command channel are dropped together");

        for _ in 0..COMMANDS_PER_CYCLE {
            if self.shutdown_signal.load(Ordering::Acquire) {
                self.begin_shutdown();
                return false;
            }
            match command_rx.try_recv() {
                Ok(command) => match command {
                    AsyncWorkerCommand::EnqueueTask { future_factory } => {
                        future_factory.into_inner()(builtins.clone(), &self.tasks);
                    }
                    AsyncWorkerCommand::Shutdown => {
                        self.begin_shutdown();

                        // Shutdown closes the command channel - no more commands can be received.
                        return false;
                    }
                },
                Err(error) => {
                    // Every initialized worker retains its own dispatcher until shutdown.
                    assert!(error.is_empty(), "async worker command channel disconnected without shutdown");
                    return false;
                }
            }
        }
        // An exhausted batch may leave commands queued; do not park before checking again.
        true
    }

    #[cfg_attr(test, mutants::skip)] // If mutated, shutdown process will never finish - will hang.
    fn begin_shutdown(&mut self) {
        Scheduler::clear_current();
        // Drop worker-local services, as they may hold references to resources that
        // block executor shutdown (via various futures, waiters, etc).
        self.builtins = None;

        // The command channel is henceforth closed. We will not receive any more commands.
        self.command_rx = None;

        self.blocking_worker.shutdown();

        // Safe executor drop synchronously cancels and releases all remaining task storage.
        self.executor = None;
    }
}

impl Drop for AsyncWorker {
    fn drop(&mut self) {
        if self.executor.is_some() {
            self.begin_shutdown();
        }
        Scheduler::clear_worker_thread();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    #[cfg(not(miri))]
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;
    #[cfg(not(miri))]
    use std::task::{Wake, Waker};
    use std::thread;

    use arty_executor::{JoinHandle as ExecutorJoinHandle, TaskOptions};
    #[cfg(not(miri))]
    use events_once::BoxedSender;
    use events_once::{Event, IntoValueError};
    use nonempty::NonEmpty;
    use testing_aids::async_test;
    use thread_aware::ThreadBuilder;
    use tick::ClockControl;

    use super::*;
    use crate::runtime::blocking_worker::BlockingPool;
    use crate::runtime::blocking_worker::blocking_worker_tests::is_blocking_worker_shutting_down;
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::thread::waiter::ThreadWaiter;
    use crate::task::execution::BoxedRemoteFutureFactory;

    trait TestTaskSetExt {
        fn add_test<F, R>(&self, future: F) -> ExecutorJoinHandle<R>
        where
            F: IntoFuture<Output = R> + 'static,
            R: 'static;
    }

    impl TestTaskSetExt for TaskSet {
        fn add_test<F, R>(&self, future: F) -> ExecutorJoinHandle<R>
        where
            F: IntoFuture<Output = R> + 'static,
            R: 'static,
        {
            self.add(future, TaskOptions::default())
        }
    }

    fn enqueue(factory: BoxedRemoteFutureFactory) -> AsyncWorkerCommand {
        AsyncWorkerCommand::EnqueueTask {
            future_factory: crate::runtime::worker::protocol::QueuedTaskFactory::new(factory),
        }
    }

    fn test_worker(
        command_tx: channel::Sender<AsyncWorkerCommand>,
        command_rx: channel::Receiver<AsyncWorkerCommand>,
        blocking_worker: Arc<BlockingWorker>,
        clock: InactiveClock,
        signal: Arc<WorkerSignal>,
    ) -> AsyncWorker {
        let sink = Sink::noop();
        let thread = ThreadBuilder::default().build(thread::current().id());
        let endpoint = WorkerEndpoint {
            command_tx,
            waker: WorkerSignal::waker(&signal),
            thread: thread.clone(),
            blocking_worker: Arc::clone(&blocking_worker),
        };
        let shutdown_signal = Arc::new(AtomicBool::new(false));
        let dispatcher = DispatcherClient::new(Arc::new(DispatcherCore::new_with_shutdown(
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint),
            sink.clone(),
            Arc::clone(&shutdown_signal),
        )));

        AsyncWorker::new(command_rx, dispatcher, thread, sink, blocking_worker, clock, signal)
    }

    fn replenishing_command(sender: channel::Sender<AsyncWorkerCommand>, processed: Arc<AtomicUsize>) -> AsyncWorkerCommand {
        enqueue(Box::new(move |_, _| {
            processed.fetch_add(1, Ordering::Relaxed);
            sender.send(replenishing_command(sender.clone(), Arc::clone(&processed))).unwrap();
        }))
    }

    #[cfg(not(miri))]
    struct GeneratedWorkerScenario {
        command_tx: channel::Sender<AsyncWorkerCommand>,
        ready_task: Option<BoxedSender<()>>,
        control: ClockControl,
        task_done: Arc<AtomicBool>,
        timer_done: Arc<AtomicBool>,
        factory_invocations: Arc<AtomicUsize>,
        forbidden_factory_invocations: Arc<AtomicUsize>,
        signal: Arc<WorkerSignal>,
        worker: AsyncWorker,
        timer_ready: bool,
        timer_advanced_by_step: bool,
        queued_shutdown: bool,
        queued_factories: usize,
        sent_factories: usize,
        progress_failure: Option<&'static str>,
    }

    #[cfg(not(miri))]
    impl GeneratedWorkerScenario {
        const MAX_OPERATIONS: usize = 12;
        const MAX_CLEANUP_STEPS: usize = Self::MAX_OPERATIONS + 8;

        fn new() -> Self {
            let (command_tx, command_rx) = channel::unbounded();
            let (ready_task, task_ready) = Event::boxed();
            let control = ClockControl::new();
            let task_done = Arc::new(AtomicBool::new(false));
            let timer_done = Arc::new(AtomicBool::new(false));
            let factory_invocations = Arc::new(AtomicUsize::new(0));
            let forbidden_factory_invocations = Arc::new(AtomicUsize::new(0));
            let signal = Arc::new(WorkerSignal::default());
            let task_observer = Arc::clone(&task_done);
            let timer_observer = Arc::clone(&timer_done);

            let worker = test_worker(
                command_tx.clone(),
                command_rx,
                BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                control.clone().into(),
                Arc::clone(&signal),
            );
            drop(worker.tasks.add_test(async move {
                task_ready.await.unwrap();
                task_observer.store(true, Ordering::Relaxed);
            }));
            let clock = worker.builtins.as_ref().unwrap().clock().clone();
            drop(worker.tasks.add_test(async move {
                clock.delay(Duration::from_secs(1)).await;
                timer_observer.store(true, Ordering::Relaxed);
            }));
            let mut scenario = Self {
                command_tx,
                ready_task: Some(ready_task),
                control,
                task_done,
                timer_done,
                factory_invocations,
                forbidden_factory_invocations,
                signal,
                worker,
                timer_ready: false,
                timer_advanced_by_step: false,
                queued_shutdown: false,
                queued_factories: 0,
                sent_factories: 0,
                progress_failure: None,
            };
            scenario.setup_step("worker stopped while polling controlled work");
            scenario
        }

        fn setup_step(&mut self, failure: &'static str) {
            if self.worker.execute_step().is_break() {
                self.progress_failure = Some(failure);
            }
        }

        fn apply(&mut self, operation: u8) {
            match operation % 5 {
                0 => self.queue_factory_batch(operation),
                1 => {
                    if let Some(ready_task) = self.ready_task.take() {
                        ready_task.send(());
                    }
                }
                2 => self.ready_timer(),
                3 => self.step(),
                _ => self.queue_shutdown(),
            }
        }

        fn queue_factory_batch(&mut self, operation: u8) {
            let batch_size = match (operation / 6) % 4 {
                0 => 1,
                1 => COMMANDS_PER_CYCLE - 1,
                2 => COMMANDS_PER_CYCLE,
                _ => COMMANDS_PER_CYCLE + 1,
            };
            for _ in 0..batch_size {
                let invocations = Arc::clone(&self.factory_invocations);
                let forbidden = Arc::clone(&self.forbidden_factory_invocations);
                let forbidden_after_queued_shutdown = self.queued_shutdown;
                if self
                    .command_tx
                    .send(enqueue(Box::new(move |_, _| {
                        invocations.fetch_add(1, Ordering::Relaxed);
                        if forbidden_after_queued_shutdown {
                            forbidden.fetch_add(1, Ordering::Relaxed);
                        }
                    })))
                    .is_err()
                {
                    break;
                }
                self.queued_factories += 1;
                self.sent_factories += 1;
            }
        }

        fn ready_timer(&mut self) {
            if !self.timer_ready {
                self.control.advance(Duration::from_secs(1));
                self.timer_ready = true;
            }
        }

        fn step(&mut self) {
            if self.worker.executor.is_none() {
                return;
            }
            WorkerSignal::waker(&self.signal).wake_by_ref();
            let full_batch = self.queued_factories >= COMMANDS_PER_CYCLE && !self.queued_shutdown;
            let task_should_progress =
                full_batch && self.ready_task.is_none() && !self.task_done.load(Ordering::Relaxed) && self.progress_failure.is_none();
            let timer_should_progress =
                full_batch && self.timer_advanced_by_step && !self.timer_done.load(Ordering::Relaxed) && self.progress_failure.is_none();
            let running = self.worker.execute_step().is_continue();
            if running && task_should_progress && !self.task_done.load(Ordering::Relaxed) {
                self.progress_failure = Some("ready task must progress between full command batches");
            }
            if running && timer_should_progress && !self.timer_done.load(Ordering::Relaxed) {
                self.progress_failure = Some("ready timer must progress between full command batches");
            }
            if full_batch {
                self.queued_factories -= COMMANDS_PER_CYCLE;
            } else if !self.queued_shutdown {
                self.queued_factories = 0;
            }
            self.timer_advanced_by_step |= self.timer_ready;
        }

        fn queue_shutdown(&mut self) {
            if !self.queued_shutdown {
                self.worker.shutdown_signal.store(true, Ordering::Release);
            }
            if !self.queued_shutdown && self.command_tx.send(AsyncWorkerCommand::Shutdown).is_ok() {
                self.queued_shutdown = true;
            }
        }

        fn finish(mut self) {
            if let Some(ready_task) = self.ready_task.take() {
                ready_task.send(());
            }
            self.ready_timer();
            self.queue_shutdown();

            let mut stopped = self.worker.executor.is_none();
            for _ in 0..Self::MAX_CLEANUP_STEPS {
                if stopped {
                    break;
                }
                WorkerSignal::waker(&self.signal).wake_by_ref();
                stopped = self.worker.execute_step().is_break();
            }
            if !stopped {
                std::mem::forget(self.worker);
                panic!("bounded cleanup must complete worker shutdown");
            }
            assert!(self.progress_failure.is_none(), "{}", self.progress_failure.unwrap_or_default());
            assert_eq!(
                self.forbidden_factory_invocations.load(Ordering::Relaxed),
                0,
                "factories queued after shutdown or pending at shutdown publication must not be invoked"
            );
            assert!(self.factory_invocations.load(Ordering::Relaxed) <= self.sent_factories);
        }
    }

    #[cfg(not(miri))]
    #[test]
    fn generated_command_sequences_preserve_progress_and_close_admission() {
        bolero::check!().for_each(|input: &[u8]| {
            let mut scenario = GeneratedWorkerScenario::new();
            for operation in input.iter().copied().take(GeneratedWorkerScenario::MAX_OPERATIONS) {
                scenario.apply(operation);
            }
            scenario.finish();
        });
    }

    #[test]
    fn a_never_empty_command_queue_allows_registered_tasks_and_timers_to_progress() {
        let (command_tx, command_rx) = channel::unbounded();
        let (proceed, progress) = Event::boxed();
        let control = ClockControl::new();
        let task_done = Arc::new(AtomicBool::new(false));
        let timer_done = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicUsize::new(0));
        let task_observer = Arc::clone(&task_done);
        let timer_observer = Arc::clone(&timer_done);
        let shutdown = command_tx.clone();

        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
            control.clone().into(),
            Arc::new(WorkerSignal::default()),
        );
        drop(worker.tasks.add_test(async move {
            progress.await.unwrap();
            task_observer.store(true, Ordering::Relaxed);
        }));
        let clock = worker.builtins.as_ref().unwrap().clock().clone();
        drop(worker.tasks.add_test(async move {
            clock.delay(Duration::from_secs(1)).await;
            timer_observer.store(true, Ordering::Relaxed);
            shutdown.send(AsyncWorkerCommand::Shutdown).unwrap();
        }));
        _ = worker.executor.as_mut().unwrap().execute_cycle();
        assert!(!task_done.load(Ordering::Relaxed));
        assert!(!timer_done.load(Ordering::Relaxed));

        command_tx
            .send(replenishing_command(command_tx.clone(), Arc::clone(&processed)))
            .unwrap();
        proceed.send(());
        control.advance(Duration::from_secs(1));
        worker.run();

        assert!(task_done.load(Ordering::Relaxed));
        assert!(timer_done.load(Ordering::Relaxed));
        assert!((COMMANDS_PER_CYCLE..=2 * COMMANDS_PER_CYCLE + 1).contains(&processed.load(Ordering::Relaxed)));
    }

    #[test]
    fn a_full_command_batch_skips_waiting_for_the_worker_signal() {
        let (command_tx, command_rx) = channel::unbounded();
        let signal = Arc::new(WorkerSignal::default());

        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
            InactiveClock::default(),
            Arc::clone(&signal),
        );
        assert!(worker.execute_step().is_continue());

        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(!signal.is_notified());

        for _ in 0..COMMANDS_PER_CYCLE {
            command_tx.send(enqueue(Box::new(|_, _| {}))).unwrap();
        }
        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(signal.is_notified());

        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
        worker.run();
    }

    #[cfg(not(miri))]
    #[test]
    fn final_timer_panic_occurs_after_executor_drop() {
        struct PanicWake;

        impl Wake for PanicWake {
            fn wake(self: std::sync::Arc<Self>) {
                panic!("final timer callback");
            }
        }

        let (command_tx, command_rx) = channel::unbounded();
        let worker = test_worker(
            command_tx.clone(),
            command_rx,
            BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );
        let clock = worker.builtins.as_ref().unwrap().clock().clone();
        let mut timer = clock.delay(Duration::from_millis(1));
        let waker = Waker::from(Arc::into_std_arc(Arc::new(PanicWake)));
        assert!(Pin::new(&mut timer).poll(&mut std::task::Context::from_waker(&waker)).is_pending());
        // Forgetting a timer is legal and avoids a second panic from the poisoned timer lock.
        std::mem::forget(timer);
        thread::sleep(Duration::from_millis(2));
        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run()));
        assert_eq!(*outcome.unwrap_err().downcast::<&str>().unwrap(), "final timer callback");
    }

    #[test]
    fn smoke_test() {
        async_test(async || {
            // Run worker, execute one remote task which awaits a nested task, then shut down.
            let (command_tx, command_rx) = channel::unbounded();
            let worker_command_tx = command_tx.clone();

            let async_worker_thread = thread::spawn(move || {
                let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());

                let signal = Arc::new(WorkerSignal::default());

                let worker = test_worker(
                    worker_command_tx,
                    command_rx,
                    Arc::clone(&blocking_worker),
                    InactiveClock::default(),
                    signal,
                );
                worker.run();

                // The async worker owns the blocking worker in our current implementation,
                // so we verify that we properly terminated it together with the async worker.
                assert!(is_blocking_worker_shutting_down(&blocking_worker));
            });

            // Signals that the task waiting on the nested task has completed.
            let (outer_completed_tx, outer_completed_rx) = Event::boxed();

            // Signals that the nested task has completed.
            let (inner_completed_tx, inner_completed_rx) = Event::boxed();

            command_tx
                .send(enqueue(Box::new({
                    move |_: Builtins, tasks: &TaskSet| {
                        drop(tasks.add_test(async move {
                            inner_completed_tx.send(());
                        }));
                        drop(tasks.add_test(async move {
                            inner_completed_rx.await.unwrap();
                            outer_completed_tx.send(());
                        }));
                    }
                })))
                .unwrap();

            // Command sent! Now wait for something to happen.
            outer_completed_rx.await.unwrap();

            // Test body completed. Now let's shut it down.
            command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

            // Wait for the worker to finish. It is harmless to continue immediately but we wait
            // just in case some errors occurred during shutdown - in which case we want to panic here.
            async_worker_thread.join().unwrap();
        });
    }

    #[test]
    fn worker_setup_can_register_new_tasks() {
        async_test(async || {
            // To verify tasks get executed.
            let (initial_completed_tx, initial_completed_rx) = Event::boxed();
            let (spawned_completed_tx, spawned_completed_rx) = Event::boxed();

            let (command_tx, command_rx) = channel::unbounded();
            let worker_command_tx = command_tx.clone();
            let worker_thread = thread::spawn(move || {
                let signal = Arc::new(WorkerSignal::default());

                let worker = test_worker(
                    worker_command_tx,
                    command_rx,
                    BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                    InactiveClock::default(),
                    signal,
                );
                drop(worker.tasks.add_test(async move { initial_completed_tx.send(()) }));

                worker.run();
            });

            command_tx
                .send(enqueue(Box::new({
                    move |_: Builtins, tasks: &TaskSet| {
                        drop(tasks.add_test(async move {
                            spawned_completed_tx.send(());
                        }));
                    }
                })))
                .unwrap();

            spawned_completed_rx.await.unwrap();
            initial_completed_rx.into_value().unwrap();

            // Tasks worked fine. Now let's shut it down.
            command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

            // Wait for the worker to finish. It is harmless to continue immediately but we wait
            // just in case some errors occurred during shutdown - in which case we want to panic here.
            worker_thread.join().unwrap();
        });
    }

    #[test]
    fn task_after_shutdown_is_ignored() {
        // It is possible to have "new task" commands in the async worker queue after shutdown
        // starts due to the asynchronous task-submission path.
        //
        // While we cannot honor these commands, we must still do something and ensure that all
        // resources are properly managed. What we do is simply drop the tasks on the floor and
        // make their remote join handles report shutdown.

        let (command_tx, command_rx) = channel::unbounded();
        let worker_command_tx = command_tx.clone();

        let async_worker_thread = thread::spawn(move || {
            let signal = Arc::new(WorkerSignal::default());

            let worker = test_worker(
                worker_command_tx,
                command_rx,
                BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                InactiveClock::default(),
                signal,
            );
            worker.run();
        });

        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

        let (completed_tx, completed_rx) = Event::boxed();

        // We ignore the result because the worker is shutting down and the channel might be closed already.
        let _ = command_tx.send(enqueue(Box::new({
            move |_: Builtins, tasks: &TaskSet| {
                drop(tasks.add_test(async move {
                    completed_tx.send(14);
                }));
            }
        })));

        // This waits for the worker to shut down.
        async_worker_thread.join().unwrap();

        let completed_result = completed_rx.into_value();
        assert!(matches!(completed_result, Err(IntoValueError::Disconnected)));
    }

    #[test]
    fn shutdown_command_cancels_registered_tasks_before_polling() {
        let (command_tx, command_rx) = channel::unbounded();
        let invoked = Arc::new(AtomicBool::new(false));
        let invoked_by_task = Arc::clone(&invoked);

        let worker = test_worker(
            command_tx.clone(),
            command_rx,
            BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );

        command_tx
            .send(enqueue(Box::new(move |_, tasks| {
                drop(tasks.add_test(async move {
                    invoked_by_task.store(true, Ordering::Relaxed);
                }));
            })))
            .unwrap();
        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

        worker.run();

        assert!(!invoked.load(Ordering::Relaxed));
    }

    #[test]
    fn shutdown_published_during_final_batch_registration_cancels_before_polling() {
        let (command_tx, command_rx) = channel::unbounded();
        let invoked = Arc::new(AtomicUsize::new(0));
        let worker = test_worker(
            command_tx.clone(),
            command_rx,
            BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );
        let shutdown_signal = Arc::clone(&worker.shutdown_signal);

        for index in 0..COMMANDS_PER_CYCLE {
            let invoked = Arc::clone(&invoked);
            let shutdown_signal = Arc::clone(&shutdown_signal);
            command_tx
                .send(enqueue(Box::new(move |_, tasks| {
                    if index + 1 == COMMANDS_PER_CYCLE {
                        shutdown_signal.store(true, Ordering::Release);
                    }
                    drop(tasks.add_test(async move {
                        invoked.fetch_add(1, Ordering::Relaxed);
                    }));
                })))
                .unwrap();
        }

        worker.run();

        assert_eq!(invoked.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn dropping_worker_without_run_cleans_up_safely() {
        let (commands, receiver) = channel::unbounded();
        let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());
        let worker = test_worker(
            commands,
            receiver,
            Arc::clone(&blocking_worker),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );

        drop(worker);

        assert!(is_blocking_worker_shutting_down(&blocking_worker));
    }

    #[test]
    fn explicit_executor_shutdown_completes_worker_shutdown() {
        let (commands, receiver) = channel::unbounded();
        let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());
        let mut worker = test_worker(
            commands.clone(),
            receiver,
            Arc::clone(&blocking_worker),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );
        assert!(worker.execute_step().is_continue());
        worker.executor.as_mut().unwrap().begin_shutdown();

        let outcome = worker.execute_step();

        assert!(outcome.is_break());
        assert!(worker.executor.is_none());
        assert!(is_blocking_worker_shutting_down(&blocking_worker));
        assert!(!worker.process_commands());
        drop(commands);
    }
}
