// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use arty_executor::{CycleOutcome, Executor, TaskSet};
use observed::Sink;
use performables::arc::Arc;
use performables::sync::channel;
use thread_aware::Thread;
use tick::SimpleClock;
use tick::runtime::{ClockDriver, InactiveClock};

use crate::io::{Io, RuntimeDriverHost};
use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::config::MonitorOptions;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::worker::protocol::AsyncWorkerCommand;
use crate::runtime::worker::signal::WorkerSignal;
use crate::task::{Builtins, Scheduler};

const COMMANDS_PER_CYCLE: usize = 256;

fn suspend_wait(deadline: Instant, now: Instant) -> Duration {
    deadline.saturating_duration_since(now)
}

fn can_suspend(cycle_outcome: CycleOutcome, batch_exhausted: bool) -> bool {
    matches!(cycle_outcome, CycleOutcome::Suspend) && !batch_exhausted
}

/// The async worker has exclusive use of one runtime thread. Multiple worker
/// threads can share a processor when the exact worker count exceeds available
/// processor capacity. The worker executes async tasks that are woken by local
/// or remote futures, incoming commands, and timers.
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
/// something calls [`request_stop()`][crate::runtime::RuntimeControl::request_stop]
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

    shutdown: Shutdown,

    // Becomes None during shutdown when the executor is dropped to cancel and release its tasks.
    executor: Option<Executor>,

    // Connected to the `executor`. Submissions after shutdown resolve to an executor-shutdown
    // JoinError rather than registering a task.
    tasks: TaskSet,

    // This drives moves the timers registered with the clock forward.
    clock_driver: ClockDriver,

    // Logical time follows controlled clocks and is used to calculate timer waits.
    time: SimpleClock,
    // System timer advancement and I/O wait accounting require precise elapsed time.
    physical_time: SimpleClock,
    signal: Arc<WorkerSignal>,
    monitor_options: MonitorOptions,
    _single_threaded: PhantomData<*const ()>,
}

impl AsyncWorker {
    #[expect(
        clippy::too_many_arguments,
        reason = "worker construction wires its independently owned runtime subsystems"
    )]
    pub(crate) fn new(
        command_rx: channel::Receiver<AsyncWorkerCommand>,
        dispatcher: DispatcherClient,
        thread: Thread,
        sink: Sink,
        io: Io,
        blocking_worker: &BlockingWorker,
        clock: InactiveClock,
        signal: Arc<WorkerSignal>,
        monitor_options: MonitorOptions,
    ) -> Self {
        let worker_waker = WorkerSignal::waker(&signal);
        let shutdown = dispatcher.shutdown().clone();
        let (clock, clock_driver) = clock.activate(worker_waker.clone());
        let time = clock.simple_clock().clone();

        let executor = Executor::builder()
            .owner_waker(worker_waker)
            .sink(blocking_worker.sink().clone())
            .thread(thread.clone())
            .build();
        let tasks = executor.tasks();

        let builtins = Builtins::new(dispatcher, clock, io, thread, sink);
        Scheduler::register_current(builtins.clone(), tasks.clone());

        Self {
            command_rx: Some(command_rx),
            builtins: Some(builtins),
            shutdown,
            executor: Some(executor),
            tasks,
            clock_driver,
            time,
            physical_time: SimpleClock::new_system(),
            signal,
            monitor_options,
            _single_threaded: PhantomData,
        }
    }

    /// Worker thread entry point, invoked by bootstrap after startup coordination completes.
    /// The configuration-facing `RuntimeBuilder` delegates construction to bootstrap.
    /// After this method returns, the thread will end.
    pub(crate) fn run(&mut self) {
        // Keep the worker-owned policy available at the boundary where monitoring
        // will be attached in a later stage.
        let _monitor_policy = self.monitor_options.policy();
        self.execute_phase();
    }

    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn execute_phase(&mut self) {
        while matches!(self.execute_step(), ControlFlow::Continue(())) {}
    }

    fn execute_step(&mut self) -> ControlFlow<()> {
        let batch_exhausted = self.process_commands();

        if matches!(self.shutdown.state(), ShutdownState::Started) {
            self.begin_shutdown();
            let now = self.timer_now();
            _ = self.clock_driver.advance_timers(now);
            return ControlFlow::Break(());
        }

        let executor = self
            .executor
            .as_mut()
            .expect("executor removal publishes shutdown, which is handled above");

        let cycle_outcome = executor.execute_cycle();

        if matches!(cycle_outcome, CycleOutcome::Shutdown) {
            // An alternate low-level owner initiated executor shutdown. Complete the surrounding
            // worker shutdown before final timer callbacks run.
            self.begin_shutdown();
            let now = self.timer_now();
            _ = self.clock_driver.advance_timers(now);
            return ControlFlow::Break(());
        }

        let advance_now = self.timer_now();
        let next_timer = self.clock_driver.advance_timers(advance_now).ok().flatten();
        if matches!(self.shutdown.state(), ShutdownState::Started) {
            self.begin_shutdown();
            let now = self.timer_now();
            _ = self.clock_driver.advance_timers(now);
            return ControlFlow::Break(());
        }
        let should_suspend = can_suspend(cycle_outcome, batch_exhausted);
        let has_primary = RuntimeDriverHost::has_primary_current();
        let max_wait = if should_suspend {
            if has_primary {
                if self.signal.take_notification() {
                    Duration::ZERO
                } else {
                    next_timer.map_or(Duration::MAX, |deadline| suspend_wait(deadline, self.time.instant()))
                }
            } else {
                Duration::ZERO
            }
        } else {
            Duration::ZERO
        };
        if RuntimeDriverHost::execute_cycle_current(max_wait).is_err() {
            self.begin_shutdown();
            let now = self.timer_now();
            _ = self.clock_driver.advance_timers(now);
            return ControlFlow::Break(());
        }

        if should_suspend && !has_primary {
            match next_timer {
                Some(deadline) => self.signal.wait(suspend_wait(deadline, self.time.instant())),
                None => self.signal.wait_for_notification(),
            }
        }

        ControlFlow::Continue(())
    }

    fn timer_now(&self) -> Instant {
        // Reading logical time preserves controlled-clock auto-advance. The precise physical read
        // prevents system timer advancement from lagging behind a deadline registered during task
        // cancellation on platforms where consecutive monotonic reads can have different precision.
        self.time.instant().max(self.physical_time.instant())
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
            if matches!(self.shutdown.state(), ShutdownState::Started) {
                self.begin_shutdown();
                return false;
            }
            match command_rx.try_recv() {
                Ok(command) => match command {
                    AsyncWorkerCommand::EnqueueTask { future_factory } => {
                        let _registered = future_factory.into_inner().register(builtins.clone(), &self.tasks);
                    }
                    AsyncWorkerCommand::AccessIo { access } => {
                        access.into_inner()(builtins);
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
        if self.executor.is_none() {
            return;
        }

        Scheduler::clear_current();
        // Drop worker-local services, as they may hold references to resources that
        // block executor shutdown (via various futures, waiters, etc).
        self.builtins = None;

        // The command channel is henceforth closed. We will not receive any more commands.
        self.command_rx = None;

        let cause = self.shutdown.cause().unwrap_or_default();
        self.shutdown.begin(cause.clone());

        // Safe executor shutdown synchronously cancels and releases all remaining task storage.
        if let Some(mut executor) = self.executor.take() {
            executor.begin_shutdown(cause);
            drop(executor);
        }
        let driver_host = RuntimeDriverHost::uninstall();
        drop(driver_host);
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    #[cfg(not(miri))]
    use std::task::Wake;
    use std::task::Waker;
    use std::thread;

    use arty_executor::{JoinHandle as ExecutorJoinHandle, ShutdownCause, TaskOptions};
    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
    #[cfg(not(miri))]
    use events_once::BoxedSender;
    use events_once::{Event, IntoValueError};
    use nonempty::NonEmpty;
    use testing_aids::{TEST_TIMEOUT, async_test};
    use thread_aware::{ThreadAware, ThreadBuilder};
    use tick::ClockControl;

    use super::*;
    use crate::runtime::blocking_worker::BlockingPool;
    use crate::runtime::blocking_worker::blocking_worker_tests::is_blocking_worker_shutting_down;
    use crate::runtime::config::MonitorPolicy;
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::thread::waiter::ThreadWaiter;
    use crate::task::execution::RemoteFutureFactory;

    fn test_blocking_worker() -> (Arc<BlockingWorker>, Shutdown) {
        let shutdown = Shutdown::default();
        (
            BlockingWorker::new(BlockingPool::new(None), Sink::noop(), shutdown.clone()),
            shutdown,
        )
    }

    #[test]
    fn suspend_budget_requires_suspend_outcome_and_non_exhausted_batch() {
        assert!(can_suspend(CycleOutcome::Suspend, false));
        assert!(!can_suspend(CycleOutcome::Suspend, true));
        assert!(!can_suspend(CycleOutcome::Continue, false));
        assert!(!can_suspend(CycleOutcome::Continue, true));
        assert!(!can_suspend(CycleOutcome::Shutdown, false));
        assert!(!can_suspend(CycleOutcome::Shutdown, true));
    }

    #[test]
    fn private_suspend_wait_uses_the_exact_timer_deadline() {
        // The exact wait calculation is private worker-loop policy; public timer wake and
        // completion behavior is covered by integration tests in `tests/time.rs`.
        let now = Instant::now();

        assert_eq!(suspend_wait(now + Duration::from_secs(2), now), Duration::from_secs(2));
        assert_eq!(suspend_wait(now + Duration::from_millis(20), now), Duration::from_millis(20));
        assert_eq!(
            suspend_wait(
                now.checked_sub(Duration::from_millis(1))
                    .expect("the current instant supports a 1 ms subtraction"),
                now
            ),
            Duration::ZERO
        );
    }

    #[test]
    fn worker_preserves_monitor_options() {
        let (command_tx, command_rx) = channel::unbounded();
        let signal = Arc::new(WorkerSignal::default());
        let worker = test_worker_with_monitor_options(
            command_tx,
            command_rx,
            test_blocking_worker(),
            InactiveClock::default(),
            signal,
            MonitorPolicy::disabled().into(),
        );

        assert!(!worker.monitor_options.policy().is_enabled());
    }

    #[test]
    fn injected_time_advances_worker_timers_without_elapsed_wall_time() {
        let (command_tx, command_rx) = channel::unbounded();
        let signal = Arc::new(WorkerSignal::default());
        let control = ClockControl::new();
        let time = control.to_simple_clock();
        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            test_blocking_worker(),
            InactiveClock::from(control.clone()),
            Arc::clone(&signal),
        );
        let clock = worker.builtins.as_ref().unwrap().clock().clone();
        let before_registration = time.instant();
        let mut delay = Box::pin(clock.delay(Duration::from_mins(1)));
        let mut context = std::task::Context::from_waker(Waker::noop());
        assert!(std::future::Future::poll(delay.as_mut(), &mut context).is_pending());
        let control = control.auto_advance_timers(true);
        WorkerSignal::waker(&signal).wake_by_ref();

        assert!(worker.execute_step().is_continue());
        let _control = control.auto_advance_timers(false);
        assert_eq!(time.instant().duration_since(before_registration), Duration::from_mins(1));
        assert!(std::future::Future::poll(delay.as_mut(), &mut context).is_ready());

        drop(delay);
        drop(clock);
        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
        worker.run();
    }

    #[cfg(not(miri))]
    #[test]
    fn worker_without_primary_waits_for_timer_or_notification() {
        let (worker_ready, worker_setup) = mpsc::channel();
        let (step_done, step_completion) = mpsc::channel();
        let worker_thread = thread::spawn(move || {
            let (command_tx, command_rx) = channel::unbounded();
            let signal = Arc::new(WorkerSignal::default());
            let mut worker = test_worker(
                command_tx.clone(),
                command_rx,
                test_blocking_worker(),
                InactiveClock::default(),
                Arc::clone(&signal),
            );
            let clock = worker.builtins.as_ref().unwrap().clock().clone();
            let mut delay = Box::pin(clock.delay(Duration::from_mins(1)));
            assert!(delay.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())).is_pending());
            signal.wait(Duration::ZERO);
            worker_ready.send(WorkerSignal::waker(&signal)).unwrap();

            assert!(worker.execute_step().is_continue());
            step_done.send(()).unwrap();

            drop(delay);
            drop(clock);
            command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
            worker.run();
        });
        let worker_waker = worker_setup.recv_timeout(TEST_TIMEOUT).unwrap();

        assert_eq!(
            step_completion.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        worker_waker.wake_by_ref();
        step_completion.recv_timeout(TEST_TIMEOUT).unwrap();
        worker_thread.join().unwrap();
    }

    #[cfg(not(miri))]
    #[test]
    fn first_cross_thread_timer_interrupts_established_idle_wait() {
        struct TimerWake(mpsc::Sender<()>);

        impl Wake for TimerWake {
            fn wake(self: std::sync::Arc<Self>) {
                _ = self.0.send(());
            }
        }

        let (wait_entered, wait_entry) = mpsc::channel();
        let (worker_ready, worker_setup) = mpsc::channel();
        let worker_thread = thread::spawn(move || {
            let (command_tx, command_rx) = channel::unbounded();
            let signal = Arc::new(WorkerSignal::with_wait_observer(wait_entered));
            let worker_waker = WorkerSignal::waker(&signal);
            let mut worker = test_worker(
                command_tx.clone(),
                command_rx,
                test_blocking_worker(),
                InactiveClock::default(),
                signal,
            );
            let clock = worker.builtins.as_ref().unwrap().clock().clone();
            worker_ready.send((command_tx, worker_waker, clock)).unwrap();
            worker.run();
        });
        let (command_tx, worker_waker, clock) = worker_setup.recv_timeout(TEST_TIMEOUT).unwrap();
        wait_entry.recv_timeout(TEST_TIMEOUT).unwrap();
        let (timer_fired, timer_completion) = mpsc::channel();
        let timer_waker = Waker::from(std::sync::Arc::new(TimerWake(timer_fired)));
        let mut delay = Box::pin(clock.delay(Duration::from_millis(1)));

        assert!(delay.as_mut().poll(&mut std::task::Context::from_waker(&timer_waker)).is_pending());
        timer_completion.recv_timeout(TEST_TIMEOUT).unwrap();

        drop(delay);
        drop(clock);
        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
        worker_waker.wake_by_ref();
        worker_thread.join().unwrap();
    }

    #[cfg(not(miri))]
    #[test]
    fn timer_callback_direct_submission_notifies_the_worker() {
        struct SpawnOnWake {
            scheduler: Scheduler,
            task_ran: Arc<AtomicBool>,
        }

        impl Wake for SpawnOnWake {
            fn wake(self: std::sync::Arc<Self>) {
                let task_ran = Arc::clone(&self.task_ran);
                drop(self.scheduler.spawn(move |builtins| async move {
                    task_ran.store(true, Ordering::Relaxed);
                    builtins.scheduler().dispatcher.stop();
                }));
            }
        }

        let timed_out = Arc::new(AtomicBool::new(false));
        let timed_out_by_worker = Arc::clone(&timed_out);
        let (worker_ready, worker_setup) = mpsc::channel();
        let (worker_done, worker_completion) = mpsc::channel();
        let worker_thread = thread::spawn(move || {
            let (command_tx, command_rx) = channel::unbounded();
            let signal = Arc::new(WorkerSignal::default());
            let control = ClockControl::new();
            let mut worker = test_worker(
                command_tx,
                command_rx,
                test_blocking_worker(),
                control.clone().into(),
                Arc::clone(&signal),
            );
            let (scheduler, clock) = {
                let builtins = worker.builtins.as_ref().unwrap();
                (builtins.scheduler().clone(), builtins.clock().clone())
            };
            let task_ran = Arc::new(AtomicBool::new(false));
            let timer_waker = Waker::from(std::sync::Arc::new(SpawnOnWake {
                scheduler,
                task_ran: Arc::clone(&task_ran),
            }));
            let mut delay = Box::pin(clock.delay(Duration::from_mins(1)));
            assert!(delay.as_mut().poll(&mut std::task::Context::from_waker(&timer_waker)).is_pending());
            worker_ready.send(WorkerSignal::waker(&signal)).unwrap();

            WorkerSignal::waker(&signal).wake_by_ref();
            assert!(worker.execute_step().is_continue());
            let _control = control.auto_advance_timers(true);
            assert!(worker.execute_step().is_continue());
            assert!(
                !timed_out_by_worker.load(Ordering::Relaxed),
                "the timer callback's direct submission must release the worker wait"
            );
            assert!(!task_ran.load(Ordering::Relaxed));

            assert!(worker.execute_step().is_break());
            assert!(task_ran.load(Ordering::Relaxed));
            worker_done.send(()).unwrap();
        });
        let worker_waker = worker_setup.recv_timeout(TEST_TIMEOUT).unwrap();

        if worker_completion.recv_timeout(TEST_TIMEOUT).is_err() {
            timed_out.store(true, Ordering::Relaxed);
            worker_waker.wake_by_ref();
        }
        worker_thread.join().unwrap();
        assert!(!timed_out.load(Ordering::Relaxed));
    }

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

    fn enqueue(factory: RemoteFutureFactory) -> AsyncWorkerCommand {
        AsyncWorkerCommand::EnqueueTask {
            future_factory: crate::runtime::worker::protocol::QueuedTaskFactory::new(factory),
        }
    }

    #[derive(Clone)]
    struct WaitRecordingContext(Arc<std::sync::Mutex<Vec<Duration>>>);

    impl ThreadAware for WaitRecordingContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for WaitRecordingContext {
        type ContextOptions = Arc<std::sync::Mutex<Vec<Duration>>>;
        type Driver = WaitRecordingDriver;

        fn context(options: Self::ContextOptions) -> Self {
            Self(options)
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            Ok(WaitRecordingDriver(Arc::clone(&self.0)))
        }
    }

    struct WaitRecordingDriver(Arc<std::sync::Mutex<Vec<Duration>>>);

    impl Driver for WaitRecordingDriver {
        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
            self.0.lock().unwrap().push(cycle.max_wait());
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    fn test_worker(
        command_tx: channel::Sender<AsyncWorkerCommand>,
        command_rx: channel::Receiver<AsyncWorkerCommand>,
        blocking: (Arc<BlockingWorker>, Shutdown),
        clock: InactiveClock,
        signal: Arc<WorkerSignal>,
    ) -> AsyncWorker {
        test_worker_with_monitor_options(command_tx, command_rx, blocking, clock, signal, MonitorOptions::default())
    }

    fn test_worker_with_monitor_options(
        command_tx: channel::Sender<AsyncWorkerCommand>,
        command_rx: channel::Receiver<AsyncWorkerCommand>,
        blocking: (Arc<BlockingWorker>, Shutdown),
        clock: InactiveClock,
        signal: Arc<WorkerSignal>,
        monitor_options: MonitorOptions,
    ) -> AsyncWorker {
        let (blocking_worker, shutdown) = blocking;
        let sink = Sink::noop();
        let thread = ThreadBuilder::default().build(thread::current().id());
        let endpoint = WorkerEndpoint {
            command_tx,
            waker: WorkerSignal::waker(&signal),
            thread: thread.clone(),
            blocking_worker: Arc::clone(&blocking_worker),
        };
        let dispatcher = DispatcherClient::new(Arc::new(DispatcherCore::new(
            ThreadWaiter::new(Vec::new()),
            NonEmpty::new(endpoint),
            sink.clone(),
            shutdown,
            crate::runtime::PanicPolicy::default(),
        )));
        let io_runtime = arty_io_host::IoRuntime::new(sink.clone());
        let io = crate::io::Io::new(io_runtime.clone(), dispatcher.clone(), thread.clone());
        let driver_waker = crate::io::RuntimeDriverHost::install(&io_runtime, dispatcher.clone(), &thread, WorkerSignal::waker(&signal));
        signal.set_driver_waker(driver_waker);

        AsyncWorker::new(
            command_rx,
            dispatcher,
            thread,
            sink,
            io,
            blocking_worker.as_ref(),
            clock,
            signal,
            monitor_options,
        )
    }

    fn replenishing_command(sender: channel::Sender<AsyncWorkerCommand>, processed: Arc<AtomicUsize>) -> AsyncWorkerCommand {
        enqueue(RemoteFutureFactory::new(move |_, _| {
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
                test_blocking_worker(),
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
            WorkerSignal::waker(&self.signal).wake_by_ref();
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
                    .send(enqueue(RemoteFutureFactory::new(move |_, _| {
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
                self.worker.shutdown.begin(ShutdownCause::default());
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
            test_blocking_worker(),
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
            test_blocking_worker(),
            InactiveClock::default(),
            Arc::clone(&signal),
        );
        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(worker.execute_step().is_continue());

        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(!signal.is_notified());

        for _ in 0..COMMANDS_PER_CYCLE {
            command_tx.send(enqueue(RemoteFutureFactory::new(|_, _| {}))).unwrap();
        }
        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(signal.is_notified());

        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
        worker.run();
    }

    #[test]
    fn a_full_command_batch_gives_the_primary_driver_no_wait_budget_and_skips_signal_wait() {
        let (command_tx, command_rx) = channel::unbounded();
        let signal = Arc::new(WorkerSignal::default());
        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            test_blocking_worker(),
            InactiveClock::default(),
            Arc::clone(&signal),
        );
        let waits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let _context = worker
            .builtins
            .as_ref()
            .unwrap()
            .io()
            .context::<WaitRecordingContext>(&Arc::clone(&waits));
        for _ in 0..COMMANDS_PER_CYCLE {
            command_tx.send(enqueue(RemoteFutureFactory::new(|_, _| {}))).unwrap();
        }
        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());

        assert!(worker.execute_step().is_continue());
        assert_eq!(*waits.lock().unwrap(), vec![Duration::ZERO]);
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
        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            test_blocking_worker(),
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
            let signal = Arc::new(WorkerSignal::default());
            let worker_signal = Arc::clone(&signal);

            let async_worker_thread = thread::spawn(move || {
                let blocking_worker = test_blocking_worker();

                let mut worker = test_worker(
                    worker_command_tx,
                    command_rx,
                    (Arc::clone(&blocking_worker.0), blocking_worker.1.clone()),
                    InactiveClock::default(),
                    worker_signal,
                );
                worker.run();

                assert!(is_blocking_worker_shutting_down(&blocking_worker.0));
            });

            // Signals that the task waiting on the nested task has completed.
            let (outer_completed_tx, outer_completed_rx) = Event::boxed();

            // Signals that the nested task has completed.
            let (inner_completed_tx, inner_completed_rx) = Event::boxed();

            command_tx
                .send(enqueue(RemoteFutureFactory::new({
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
            WorkerSignal::waker(&signal).wake_by_ref();

            // Command sent! Now wait for something to happen.
            outer_completed_rx.await.unwrap();

            // Test body completed. Now let's shut it down.
            command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
            WorkerSignal::waker(&signal).wake_by_ref();

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
            let signal = Arc::new(WorkerSignal::default());
            let worker_signal = Arc::clone(&signal);
            let worker_thread = thread::spawn(move || {
                let mut worker = test_worker(
                    worker_command_tx,
                    command_rx,
                    test_blocking_worker(),
                    InactiveClock::default(),
                    worker_signal,
                );
                drop(worker.tasks.add_test(async move { initial_completed_tx.send(()) }));

                worker.run();
            });

            command_tx
                .send(enqueue(RemoteFutureFactory::new({
                    move |_: Builtins, tasks: &TaskSet| {
                        drop(tasks.add_test(async move {
                            spawned_completed_tx.send(());
                        }));
                    }
                })))
                .unwrap();
            WorkerSignal::waker(&signal).wake_by_ref();

            spawned_completed_rx.await.unwrap();
            initial_completed_rx.into_value().unwrap();

            // Tasks worked fine. Now let's shut it down.
            command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
            WorkerSignal::waker(&signal).wake_by_ref();

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
        let signal = Arc::new(WorkerSignal::default());
        let worker_signal = Arc::clone(&signal);

        let async_worker_thread = thread::spawn(move || {
            let mut worker = test_worker(
                worker_command_tx,
                command_rx,
                test_blocking_worker(),
                InactiveClock::default(),
                worker_signal,
            );
            worker.run();
        });

        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
        WorkerSignal::waker(&signal).wake_by_ref();

        let (completed_tx, completed_rx) = Event::boxed();

        // We ignore the result because the worker is shutting down and the channel might be closed already.
        let _ = command_tx.send(enqueue(RemoteFutureFactory::new({
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

        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            test_blocking_worker(),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );

        command_tx
            .send(enqueue(RemoteFutureFactory::new(move |_, tasks| {
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
    fn repeated_shutdown_cleanup_is_idempotent() {
        let (command_tx, command_rx) = channel::unbounded();
        let mut worker = test_worker(
            command_tx,
            command_rx,
            test_blocking_worker(),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );

        worker.begin_shutdown();
        worker.begin_shutdown();
    }

    #[test]
    fn published_shutdown_discards_queued_io_access_without_invoking_it() {
        struct DropSignal(mpsc::Sender<()>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                _ = self.0.send(());
            }
        }

        let (command_tx, command_rx) = channel::unbounded();
        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            test_blocking_worker(),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );
        let invoked = Arc::new(AtomicBool::new(false));
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let drop_signal = DropSignal(dropped_tx);
        command_tx
            .send(AsyncWorkerCommand::AccessIo {
                access: crate::runtime::worker::protocol::QueuedIoAccess::new(Box::new({
                    let invoked = Arc::clone(&invoked);
                    move |_| {
                        let _ = &drop_signal;
                        invoked.store(true, Ordering::Release);
                    }
                })),
            })
            .unwrap();
        worker.shutdown.begin(ShutdownCause::default());

        worker.run();

        dropped_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        assert!(!invoked.load(Ordering::Acquire));
    }

    #[test]
    fn shutdown_published_during_final_batch_registration_cancels_before_polling() {
        let (command_tx, command_rx) = channel::unbounded();
        let invoked = Arc::new(AtomicUsize::new(0));
        let mut worker = test_worker(
            command_tx.clone(),
            command_rx,
            test_blocking_worker(),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );
        let shutdown = worker.shutdown.clone();

        for index in 0..COMMANDS_PER_CYCLE {
            let invoked = Arc::clone(&invoked);
            let shutdown = shutdown.clone();
            command_tx
                .send(enqueue(RemoteFutureFactory::new(move |_, tasks| {
                    if index + 1 == COMMANDS_PER_CYCLE {
                        shutdown.begin(ShutdownCause::default());
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
        let blocking_worker = test_blocking_worker();
        let worker = test_worker(
            commands,
            receiver,
            (Arc::clone(&blocking_worker.0), blocking_worker.1.clone()),
            InactiveClock::default(),
            Arc::new(WorkerSignal::default()),
        );

        drop(worker);

        assert!(is_blocking_worker_shutting_down(&blocking_worker.0));
    }

    #[test]
    fn explicit_executor_shutdown_completes_worker_shutdown() {
        let (commands, receiver) = channel::unbounded();
        let blocking_worker = test_blocking_worker();
        let signal = Arc::new(WorkerSignal::default());
        let mut worker = test_worker(
            commands.clone(),
            receiver,
            (Arc::clone(&blocking_worker.0), blocking_worker.1.clone()),
            InactiveClock::default(),
            Arc::clone(&signal),
        );
        WorkerSignal::waker(&signal).wake_by_ref();
        assert!(worker.execute_step().is_continue());
        worker.executor.as_mut().unwrap().begin_shutdown(ShutdownCause::default());

        let outcome = worker.execute_step();

        assert!(outcome.is_break());
        assert!(worker.executor.is_none());
        assert!(is_blocking_worker_shutting_down(&blocking_worker.0));
        assert!(!worker.process_commands());
        drop(commands);
    }
}
