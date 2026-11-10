// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Asynchronous worker execution loop.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use arty_executor::{CycleOutcome, Executor, TaskSet};
use performables::arc::Arc;
use thread_aware::Thread;
use tick::SimpleClock;
use tick::runtime::{ClockDriver, InactiveClock};

use crate::io::{Io, RuntimeDriverHost};
use crate::runtime::config::MonitorOptions;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::worker::asynchronous::{CommandReceiver, LocalWorker, Signal, SpawnCommand, SystemCommand};
#[cfg(test)]
use crate::runtime::worker::asynchronous::{CommandSender, command_channel};
#[cfg(test)]
use crate::runtime::worker::blocking;
use crate::task::Context;

const COMMANDS_PER_CYCLE: usize = 256;

fn suspend_wait(deadline: Instant, now: Instant) -> Duration {
    deadline.saturating_duration_since(now)
}

fn can_suspend(cycle_outcome: &CycleOutcome, batch_exhausted: bool) -> bool {
    matches!(cycle_outcome, CycleOutcome::Suspend) && !batch_exhausted
}

/// The async worker has exclusive use of one runtime thread. Multiple worker
/// threads can share a processor when the exact worker count exceeds available
/// processor capacity. The worker executes async tasks that are woken by local
/// or remote futures, incoming commands, and timers.
///
/// This type contains the internal worker state. Runtime components send work
/// and signals to it through channels.
///
/// # Ownership
///
/// This type is exclusively owned by the thread entry point.
///
/// # Lifecycle
///
/// The worker has these lifecycle stages:
///
/// 1. Startup: Bootstrap creates the thread and worker from the
///    [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) configuration. Bootstrap
///    and the worker coordinate initialization until the worker is ready.
/// 2. Running: The worker processes system commands, registers queued tasks,
///    polls tasks, advances timers, and waits for commands. It drains system
///    commands before processing a bounded batch of task submissions.
/// 3. Shutting down: The worker rejects queued and new submissions, stops
///    blocking work, cancels async tasks, and runs final timer callbacks.
/// 4. Dead: The worker completed shutdown and its thread terminated.
///
/// A running worker stops after
/// [`RuntimeControl::request_stop`](crate::runtime::RuntimeControl::request_stop),
/// `Runtime::stop`, or `Runtime` destruction.
///
/// # Thread safety
///
/// This type is single-threaded. It only exists on its designated worker thread.
#[derive(Debug)]
pub(crate) struct Worker {
    state: WorkerState,

    shutdown: Shutdown,

    // Connected to the `executor`. Submissions after shutdown resolve to an executor-shutdown
    // JoinError rather than registering a task.
    tasks: TaskSet,

    // This drives moves the timers registered with the clock forward.
    clock_driver: ClockDriver,

    // Cycle-local timestamps and wait budgets use precise system time.
    cycle_clock: SimpleClock,
    signal: Arc<Signal>,
    monitor_options: MonitorOptions,
    _single_threaded: PhantomData<*const ()>,
}

#[derive(Debug)]
enum WorkerState {
    Ready {
        commands: CommandReceiver,
        context: Context,
        executor: Executor,
    },
    Shutdown,
}

impl WorkerState {
    #[cfg_attr(coverage_nightly, coverage(off))] // Unreachable guard for a violated invariant.
    #[cfg_attr(test, mutants::skip)] // Unreachable guard for a violated invariant.
    fn ready_executor(&mut self) -> &mut Executor {
        let Self::Ready { executor, .. } = self else {
            unreachable!("process_system_commands returns early unless the worker is ready, and only finish_shutdown leaves that state");
        };
        executor
    }
}

impl Worker {
    #[expect(
        clippy::too_many_arguments,
        reason = "worker construction wires its independently owned runtime subsystems"
    )]
    pub(crate) fn new(
        commands: CommandReceiver,
        dispatcher: DispatcherClient,
        thread: Thread,
        io: Io,
        clock: InactiveClock,
        signal: Arc<Signal>,
        monitor_options: MonitorOptions,
        panic_policy: crate::runtime::PanicPolicy,
    ) -> Self {
        let worker_waker = Signal::waker(&signal);
        let shutdown = dispatcher.shutdown().clone();
        let (clock, clock_driver) = clock.activate(worker_waker.clone());
        let cycle_clock = SimpleClock::new_system();

        let executor = Executor::builder()
            .owner_waker(worker_waker)
            .thread(thread.clone())
            .panic_policy(panic_policy.into_executor_policy())
            .build();
        let tasks = executor.tasks();
        let executor_shutdown = executor.shutdown();

        let context = Context::new(dispatcher, clock, io, thread);
        LocalWorker::register_scheduler(context.clone(), tasks.clone(), executor_shutdown);

        Self {
            state: WorkerState::Ready {
                commands,
                context,
                executor,
            },
            shutdown,
            tasks,
            clock_driver,
            cycle_clock,
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
        let action = self.prepare_cycle();
        let max_wait = match action {
            CycleAction::Shutdown => return self.finish_shutdown(),
            CycleAction::WaitForIo(deadline) => self.io_wait(deadline),
            CycleAction::Poll | CycleAction::WaitForSignal(_) => Duration::ZERO,
        };

        if RuntimeDriverHost::execute_cycle_current(max_wait).is_err() {
            return self.finish_shutdown();
        }

        if let CycleAction::WaitForSignal(deadline) = action {
            match deadline {
                Some(deadline) => self.signal.wait(self.wait_until(deadline)),
                None => self.signal.wait_for_notification(),
            }
        }

        ControlFlow::Continue(())
    }

    fn prepare_cycle(&mut self) -> CycleAction {
        let Some(system_batch_exhausted) = self.process_system_commands() else {
            return CycleAction::Shutdown;
        };

        let spawn_batch_exhausted = self.process_spawn_commands();
        let batch_exhausted = system_batch_exhausted || spawn_batch_exhausted;

        if matches!(self.shutdown.state(), ShutdownState::Started) {
            return CycleAction::Shutdown;
        }

        let cycle_outcome = self.state.ready_executor().execute_cycle();

        if let CycleOutcome::Shutdown(cause) = cycle_outcome {
            self.shutdown.begin(cause);
            return CycleAction::Shutdown;
        }

        let cycle_finished = self.cycle_clock.instant();
        let next_timer = self.clock_driver.advance_timers(cycle_finished).ok().flatten();
        if matches!(self.shutdown.state(), ShutdownState::Started) {
            return CycleAction::Shutdown;
        }

        if !can_suspend(&cycle_outcome, batch_exhausted) {
            CycleAction::Poll
        } else if RuntimeDriverHost::has_primary_current() {
            CycleAction::WaitForIo(next_timer)
        } else {
            CycleAction::WaitForSignal(next_timer)
        }
    }

    fn io_wait(&self, deadline: Option<Instant>) -> Duration {
        if self.signal.take_notification() {
            return Duration::ZERO;
        }

        deadline.map_or(Duration::MAX, |deadline| self.wait_until(deadline))
    }

    fn wait_until(&self, deadline: Instant) -> Duration {
        suspend_wait(deadline, self.cycle_clock.instant())
    }

    fn finish_shutdown(&mut self) -> ControlFlow<()> {
        self.begin_shutdown();
        // Final task cancellation can register a timer after the cycle sample.
        let now = self.cycle_clock.instant();
        _ = self.clock_driver.advance_timers(now);
        ControlFlow::Break(())
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing requires a fine level of control over what is in the channel, which is too bothersome just for mutation testing.
    fn process_system_commands(&self) -> Option<bool> {
        let WorkerState::Ready { commands, context, .. } = &self.state else {
            return None;
        };

        let mut processed = 0_usize;
        for command in commands.system_commands().take(COMMANDS_PER_CYCLE) {
            if matches!(self.shutdown.state(), ShutdownState::Started) {
                return None;
            }
            match command {
                SystemCommand::AccessIo { access } => {
                    access.into_inner()(context);
                }
                SystemCommand::Shutdown => {
                    return None;
                }
            }
            processed = processed.saturating_add(1);
        }
        Some(processed == COMMANDS_PER_CYCLE)
    }

    #[cfg_attr(test, mutants::skip)] // If mutated, spawned tasks are never registered or the worker spins - will hang.
    fn process_spawn_commands(&self) -> bool {
        let WorkerState::Ready { commands, context, .. } = &self.state else {
            return false;
        };

        // Registration does not invoke user factories. If shutdown starts during
        // this batch, prepare_cycle stops before polling the registered tasks.
        let mut processed = 0_usize;
        for SpawnCommand { future_factory } in commands.spawn_commands().take(COMMANDS_PER_CYCLE) {
            let _registered = future_factory.into_inner().register(context.clone(), &self.tasks);
            processed = processed.saturating_add(1);
        }
        processed == COMMANDS_PER_CYCLE
    }

    #[cfg_attr(test, mutants::skip)] // If mutated, shutdown process will never finish - will hang.
    fn begin_shutdown(&mut self) {
        let WorkerState::Ready {
            commands,
            context,
            executor,
        } = std::mem::replace(&mut self.state, WorkerState::Shutdown)
        else {
            return;
        };

        let cause = context.dispatcher().stop(self.shutdown.cause().unwrap_or_default());

        LocalWorker::clear_scheduler();
        // Both command channels are now closed. No more commands can arrive.
        drop(commands);
        // Drop worker-local services before the executor. Their futures and
        // waiters can hold resources that block executor shutdown.
        drop(context);

        // Safe executor shutdown synchronously cancels and releases all remaining task storage.
        executor.shutdown().trigger(cause);
        drop(executor);
        let driver_host = RuntimeDriverHost::uninstall();
        drop(driver_host);
    }
}

impl Drop for Worker {
    #[cfg_attr(test, mutants::skip)] // Worker-owned services must retire before the thread can finish shutdown.
    fn drop(&mut self) {
        if matches!(self.state, WorkerState::Ready { .. }) {
            self.begin_shutdown();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CycleAction {
    Poll,
    WaitForIo(Option<Instant>),
    WaitForSignal(Option<Instant>),
    Shutdown,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    #[cfg(not(miri))]
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    #[cfg(not(miri))]
    use std::task::Wake;
    use std::task::Waker;
    use std::thread;

    use arty_executor::{ShutdownCause, TaskCompletion, TaskOptions, TaskPanicPayload, discard_task_panic_payload};
    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
    #[cfg(not(miri))]
    use events_once::BoxedSender;
    use events_once::{Event, IntoValueError};
    use nonempty::NonEmpty;
    use testing_aids::{TEST_TIMEOUT, async_test};
    use thread_aware::{ThreadAware, ThreadBuilder};
    use tick::ClockControl;

    use super::*;
    use crate::runtime::config::MonitorPolicy;
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::worker::asynchronous::{QueuedFactory, QueuedIoAccess};
    use crate::task::preparation::RemoteFutureFactory;

    fn test_blocking_worker() -> (Arc<blocking::Worker>, Shutdown) {
        let shutdown = Shutdown::default();
        (
            blocking::Worker::new(blocking::Pool::new(None), shutdown.clone(), blocking::PanicPolicy::Ignore),
            shutdown,
        )
    }

    trait TestTaskSetExt {
        fn add_test<F, R>(&self, future: F)
        where
            F: IntoFuture<Output = R> + 'static,
            R: 'static;
    }

    impl TestTaskSetExt for TaskSet {
        fn add_test<F, R>(&self, future: F)
        where
            F: IntoFuture<Output = R> + 'static,
            R: 'static,
        {
            self.add(future, TaskOptions::default(), DiscardCompletion);
        }
    }

    fn worker_context(worker: &Worker) -> &Context {
        let WorkerState::Ready { context, .. } = &worker.state else {
            panic!("test requires a running worker");
        };
        context
    }

    fn worker_executor(worker: &Worker) -> &Executor {
        let WorkerState::Ready { executor, .. } = &worker.state else {
            panic!("test requires a running worker");
        };
        executor
    }

    fn worker_executor_mut(worker: &mut Worker) -> &mut Executor {
        let WorkerState::Ready { executor, .. } = &mut worker.state else {
            panic!("test requires a running worker");
        };
        executor
    }

    fn worker_is_shutdown(worker: &Worker) -> bool {
        matches!(worker.state, WorkerState::Shutdown)
    }

    #[derive(Debug)]
    struct DiscardCompletion;

    impl<R: 'static> TaskCompletion<R> for DiscardCompletion {
        fn completed(self, result: R) {
            if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(result))) {
                discard_task_panic_payload(panic);
            }
        }

        fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
            discard_task_panic_payload(panic);
        }

        fn cancelled(self, _cause: ShutdownCause) {}
    }

    fn enqueue(factory: RemoteFutureFactory) -> SpawnCommand {
        SpawnCommand {
            future_factory: QueuedFactory::new(factory),
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

        fn new(options: Self::ContextOptions) -> Self {
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
        commands: (CommandSender, CommandReceiver),
        blocking: (Arc<blocking::Worker>, Shutdown),
        clock: InactiveClock,
        signal: Arc<Signal>,
    ) -> Worker {
        test_worker_with_monitor_options(commands, blocking, clock, signal, MonitorOptions::default())
    }

    fn test_worker_with_monitor_options(
        commands: (CommandSender, CommandReceiver),
        blocking: (Arc<blocking::Worker>, Shutdown),
        clock: InactiveClock,
        signal: Arc<Signal>,
        monitor_options: MonitorOptions,
    ) -> Worker {
        let (blocking_worker, shutdown) = blocking;
        let (command_tx, command_rx) = commands;
        let thread = ThreadBuilder::default().build(thread::current().id());
        let endpoint = WorkerEndpoint {
            commands: command_tx,
            waker: Signal::waker(&signal),
            thread: thread.clone(),
            blocking_worker: Arc::clone(&blocking_worker),
        };
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(endpoint), shutdown));
        let io_runtime = arty_io_host::IoRuntime::new(observed::Sink::noop());
        let io = crate::io::Io::new(io_runtime.clone(), dispatcher.clone(), thread.clone());
        if !LocalWorker::is_current() {
            LocalWorker::attach();
        }
        let driver_waker = crate::io::RuntimeDriverHost::install(&io_runtime, dispatcher.clone(), &thread, Signal::waker(&signal));
        signal.set_driver_waker(driver_waker);

        Worker::new(
            command_rx,
            dispatcher,
            thread,
            io,
            clock,
            signal,
            monitor_options,
            crate::runtime::PanicPolicy::default(),
        )
    }

    fn replenishing_command(sender: CommandSender, processed: Arc<AtomicUsize>) -> SpawnCommand {
        enqueue(RemoteFutureFactory::new(move |_, _| {
            processed.fetch_add(1, Ordering::Relaxed);
            sender
                .send_spawn(replenishing_command(sender.clone(), Arc::clone(&processed)))
                .unwrap();
        }))
    }

    #[cfg(not(miri))]
    struct GeneratedWorkerScenario {
        commands: CommandSender,
        ready_task: Option<BoxedSender<()>>,
        control: ClockControl,
        task_done: Arc<AtomicBool>,
        timer_done: Arc<AtomicBool>,
        factory_invocations: Arc<AtomicUsize>,
        forbidden_factory_invocations: Arc<AtomicUsize>,
        signal: Arc<Signal>,
        worker: Worker,
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
            let (commands, command_rx) = command_channel();
            let (ready_task, task_ready) = Event::boxed();
            let control = ClockControl::new();
            let task_done = Arc::new(AtomicBool::new(false));
            let timer_done = Arc::new(AtomicBool::new(false));
            let factory_invocations = Arc::new(AtomicUsize::new(0));
            let forbidden_factory_invocations = Arc::new(AtomicUsize::new(0));
            let signal = Arc::new(Signal::default());
            let task_observer = Arc::clone(&task_done);
            let timer_observer = Arc::clone(&timer_done);

            let worker = test_worker(
                (commands.clone(), command_rx),
                test_blocking_worker(),
                control.clone().into(),
                Arc::clone(&signal),
            );
            worker.tasks.add_test(async move {
                task_ready.await.unwrap();
                task_observer.store(true, Ordering::Relaxed);
            });
            let clock = worker_context(&worker).clock().clone();
            worker.tasks.add_test(async move {
                clock.delay(Duration::from_secs(1)).await;
                timer_observer.store(true, Ordering::Relaxed);
            });
            let mut scenario = Self {
                commands,
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
            Signal::waker(&self.signal).wake_by_ref();
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
                    .commands
                    .send_spawn(enqueue(RemoteFutureFactory::new(move |_, _| {
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
            if worker_is_shutdown(&self.worker) {
                return;
            }
            Signal::waker(&self.signal).wake_by_ref();
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
            if !self.queued_shutdown && self.commands.send_system(SystemCommand::Shutdown).is_ok() {
                self.queued_shutdown = true;
            }
        }

        fn finish(mut self) {
            if let Some(ready_task) = self.ready_task.take() {
                ready_task.send(());
            }
            self.ready_timer();
            self.queue_shutdown();

            let mut stopped = worker_is_shutdown(&self.worker);
            for _ in 0..Self::MAX_CLEANUP_STEPS {
                if stopped {
                    break;
                }
                Signal::waker(&self.signal).wake_by_ref();
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

    #[test]
    fn suspend_budget_requires_suspend_outcome_and_non_exhausted_batch() {
        assert!(can_suspend(&CycleOutcome::Suspend, false));
        assert!(!can_suspend(&CycleOutcome::Suspend, true));
        assert!(!can_suspend(&CycleOutcome::Continue, false));
        assert!(!can_suspend(&CycleOutcome::Continue, true));
        assert!(!can_suspend(&CycleOutcome::Shutdown(ShutdownCause::default()), false));
        assert!(!can_suspend(&CycleOutcome::Shutdown(ShutdownCause::default()), true));
    }

    #[test]
    fn system_commands_precede_earlier_spawn_submissions() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        for index in 2..4 {
            let order = Arc::clone(&order);
            commands
                .send_spawn(enqueue(RemoteFutureFactory::new(move |_, _| {
                    order.lock().unwrap().push(index);
                })))
                .unwrap();
        }
        for index in 0..2 {
            let order = Arc::clone(&order);
            commands
                .send_system(SystemCommand::AccessIo {
                    access: QueuedIoAccess::new(Box::new(move |_| {
                        order.lock().unwrap().push(index);
                    })),
                })
                .unwrap();
        }

        _ = worker.prepare_cycle();

        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn system_batch_precedes_spawning_and_allows_task_progress() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let processed = Arc::new(AtomicUsize::new(0));
        let spawned_after = Arc::new(AtomicUsize::new(0));
        let task_polled = Arc::new(AtomicBool::new(false));
        for _ in 0..=COMMANDS_PER_CYCLE {
            let processed = Arc::clone(&processed);
            commands
                .send_system(SystemCommand::AccessIo {
                    access: QueuedIoAccess::new(Box::new(move |_| {
                        processed.fetch_add(1, Ordering::Relaxed);
                    })),
                })
                .unwrap();
        }
        commands
            .send_spawn(enqueue(RemoteFutureFactory::new({
                let processed = Arc::clone(&processed);
                let spawned_after = Arc::clone(&spawned_after);
                move |_, _| {
                    spawned_after.store(processed.load(Ordering::Relaxed), Ordering::Relaxed);
                }
            })))
            .unwrap();
        worker.tasks.add_test({
            let task_polled = Arc::clone(&task_polled);
            async move {
                task_polled.store(true, Ordering::Relaxed);
            }
        });

        _ = worker.prepare_cycle();

        assert_eq!(processed.load(Ordering::Relaxed), COMMANDS_PER_CYCLE);
        assert_eq!(spawned_after.load(Ordering::Relaxed), COMMANDS_PER_CYCLE);
        assert!(task_polled.load(Ordering::Relaxed));

        _ = worker.prepare_cycle();

        assert_eq!(processed.load(Ordering::Relaxed), COMMANDS_PER_CYCLE + 1);
    }

    #[test]
    fn system_commands_arriving_during_processing_run_in_the_same_cycle() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let processed = Arc::new(AtomicUsize::new(0));
        let first_processed = Arc::clone(&processed);
        let follow_up_commands = commands.clone();
        commands
            .send_system(SystemCommand::AccessIo {
                access: QueuedIoAccess::new(Box::new(move |_| {
                    first_processed.fetch_add(1, Ordering::Relaxed);
                    let second_processed = Arc::clone(&first_processed);
                    follow_up_commands
                        .send_system(SystemCommand::AccessIo {
                            access: QueuedIoAccess::new(Box::new(move |_| {
                                second_processed.fetch_add(1, Ordering::Relaxed);
                            })),
                        })
                        .unwrap();
                })),
            })
            .unwrap();
        let task_polled = Arc::new(AtomicBool::new(false));
        worker.tasks.add_test({
            let task_polled = Arc::clone(&task_polled);
            async move {
                task_polled.store(true, Ordering::Relaxed);
            }
        });

        _ = worker.prepare_cycle();

        assert_eq!(processed.load(Ordering::Relaxed), 2);
        assert!(task_polled.load(Ordering::Relaxed));
    }

    #[test]
    fn shutdown_command_discards_an_earlier_spawn_backlog() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let invocations = Arc::new(AtomicUsize::new(0));
        for _ in 0..=COMMANDS_PER_CYCLE {
            let invocations = Arc::clone(&invocations);
            commands
                .send_spawn(enqueue(RemoteFutureFactory::new(move |_, _| {
                    invocations.fetch_add(1, Ordering::Relaxed);
                })))
                .unwrap();
        }
        let join = worker_context(&worker).dispatcher().spawn(|_| async {});
        commands.send_system(SystemCommand::Shutdown).unwrap();

        assert!(worker.execute_step().is_break());

        assert_eq!(invocations.load(Ordering::Relaxed), 0);
        let mut join = std::pin::pin!(join);
        let std::task::Poll::Ready(Err(error)) = join.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())) else {
            panic!("discarded submission must report shutdown");
        };
        assert!(error.is_shutdown());
        assert!(commands.send_system(SystemCommand::Shutdown).is_err());
        assert!(commands.send_spawn(enqueue(RemoteFutureFactory::new(|_, _| {}))).is_err());
    }

    #[test]
    fn private_suspend_wait_uses_current_time() {
        // The exact wait calculation is private worker-loop policy; public timer wake and
        // completion behavior is covered by integration tests in `tests/time.rs`.
        let now = Instant::now();

        assert_eq!(
            suspend_wait(now + Duration::from_secs(2), now + Duration::from_secs(1)),
            Duration::from_secs(1)
        );
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
        let (commands, command_rx) = command_channel();
        let signal = Arc::new(Signal::default());
        let worker = test_worker_with_monitor_options(
            (commands, command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            signal,
            MonitorPolicy::disabled().into(),
        );

        assert!(!worker.monitor_options.policy().is_enabled());
    }

    #[cfg(not(miri))]
    #[test]
    fn worker_without_primary_waits_for_timer_or_notification() {
        let (worker_ready, worker_setup) = mpsc::channel();
        let (step_done, step_completion) = mpsc::channel();
        let worker_thread = thread::spawn(move || {
            let (commands, command_rx) = command_channel();
            let signal = Arc::new(Signal::default());
            let mut worker = test_worker(
                (commands.clone(), command_rx),
                test_blocking_worker(),
                InactiveClock::new_isolated(),
                Arc::clone(&signal),
            );
            let clock = worker_context(&worker).clock().clone();
            let mut delay = Box::pin(clock.delay(Duration::from_mins(1)));
            assert!(delay.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())).is_pending());
            signal.wait(Duration::ZERO);
            worker_ready.send(Signal::waker(&signal)).unwrap();

            assert!(worker.execute_step().is_continue());
            step_done.send(()).unwrap();

            drop(delay);
            drop(clock);
            commands.send_system(SystemCommand::Shutdown).unwrap();
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
            let (commands, command_rx) = command_channel();
            let signal = Arc::new(Signal::with_wait_observer(wait_entered));
            let worker_waker = Signal::waker(&signal);
            let mut worker = test_worker(
                (commands.clone(), command_rx),
                test_blocking_worker(),
                InactiveClock::new_isolated(),
                signal,
            );
            let clock = worker_context(&worker).clock().clone();
            worker_ready.send((commands, worker_waker, clock)).unwrap();
            worker.run();
        });
        let (commands, worker_waker, clock) = worker_setup.recv_timeout(TEST_TIMEOUT).unwrap();
        wait_entry.recv_timeout(TEST_TIMEOUT).unwrap();
        let (timer_fired, timer_completion) = mpsc::channel();
        let timer_waker = Waker::from(std::sync::Arc::new(TimerWake(timer_fired)));
        let mut delay = Box::pin(clock.delay(Duration::from_millis(1)));

        assert!(delay.as_mut().poll(&mut std::task::Context::from_waker(&timer_waker)).is_pending());
        timer_completion.recv_timeout(TEST_TIMEOUT).unwrap();

        drop(delay);
        drop(clock);
        commands.send_system(SystemCommand::Shutdown).unwrap();
        worker_waker.wake_by_ref();
        worker_thread.join().unwrap();
    }

    #[cfg(not(miri))]
    #[test]
    fn timer_callback_direct_submission_notifies_worker() {
        struct SpawnOnWake {
            scheduler: crate::task::Scheduler,
            invoked: Arc<AtomicBool>,
        }

        impl Wake for SpawnOnWake {
            fn wake(self: std::sync::Arc<Self>) {
                let invoked = Arc::clone(&self.invoked);
                drop(self.scheduler.spawn(move |_| async move {
                    invoked.store(true, Ordering::Release);
                }));
            }
        }

        let control = ClockControl::new();
        let (commands, command_rx) = command_channel();
        let signal = Arc::new(Signal::default());
        let mut worker = test_worker(
            (commands, command_rx),
            test_blocking_worker(),
            control.clone().into(),
            Arc::clone(&signal),
        );
        let clock = worker_context(&worker).clock().clone();
        let invoked = Arc::new(AtomicBool::new(false));
        let callback_waker = Waker::from(std::sync::Arc::new(SpawnOnWake {
            scheduler: worker_context(&worker).scheduler().clone(),
            invoked: Arc::clone(&invoked),
        }));
        let mut delay = Box::pin(clock.delay(Duration::from_secs(1)));
        assert!(
            delay
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&callback_waker))
                .is_pending()
        );
        while signal.take_notification() {}

        control.advance(Duration::from_secs(1));

        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(invoked.load(Ordering::Acquire));
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
        let (commands, command_rx) = command_channel();
        let (proceed, progress) = Event::boxed();
        let control = ClockControl::new();
        let task_done = Arc::new(AtomicBool::new(false));
        let timer_done = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicUsize::new(0));
        let task_observer = Arc::clone(&task_done);
        let timer_observer = Arc::clone(&timer_done);
        let shutdown = commands.clone();

        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            control.clone().into(),
            Arc::new(Signal::default()),
        );
        worker.tasks.add_test(async move {
            progress.await.unwrap();
            task_observer.store(true, Ordering::Relaxed);
        });
        let clock = worker_context(&worker).clock().clone();
        worker.tasks.add_test(async move {
            clock.delay(Duration::from_secs(1)).await;
            timer_observer.store(true, Ordering::Relaxed);
            shutdown.send_system(SystemCommand::Shutdown).unwrap();
        });
        _ = worker_executor_mut(&mut worker).execute_cycle();
        assert!(!task_done.load(Ordering::Relaxed));
        assert!(!timer_done.load(Ordering::Relaxed));

        commands
            .send_spawn(replenishing_command(commands.clone(), Arc::clone(&processed)))
            .unwrap();
        proceed.send(());
        control.advance(Duration::from_secs(1));
        worker.run();

        assert!(task_done.load(Ordering::Relaxed));
        assert!(timer_done.load(Ordering::Relaxed));
        assert!((COMMANDS_PER_CYCLE..=2 * COMMANDS_PER_CYCLE + 1).contains(&processed.load(Ordering::Relaxed)));
    }

    #[test]
    fn same_worker_child_progresses_without_a_redundant_worker_notification() {
        let (commands, command_rx) = command_channel();
        let signal = Arc::new(Signal::default());
        let mut worker = test_worker(
            (commands, command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::clone(&signal),
        );
        let child_ran = Arc::new(AtomicBool::new(false));
        let child_observer = Arc::clone(&child_ran);
        let scheduler = worker_context(&worker).scheduler().clone();
        worker.tasks.add_test(async move {
            drop(scheduler.spawn(move |_| async move {
                child_observer.store(true, Ordering::Relaxed);
            }));
        });

        assert!(worker.execute_step().is_continue());
        assert!(!signal.is_notified());
        assert!(!child_ran.load(Ordering::Relaxed));

        Signal::waker(&signal).wake_by_ref();
        assert!(worker.execute_step().is_continue());
        assert!(child_ran.load(Ordering::Relaxed));
    }

    #[test]
    fn a_full_command_batch_skips_waiting_for_the_worker_signal() {
        let (commands, command_rx) = command_channel();
        let signal = Arc::new(Signal::default());

        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::clone(&signal),
        );
        Signal::waker(&signal).wake_by_ref();
        assert!(worker.execute_step().is_continue());

        Signal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(!signal.is_notified());

        for _ in 0..COMMANDS_PER_CYCLE {
            commands.send_spawn(enqueue(RemoteFutureFactory::new(|_, _| {}))).unwrap();
        }
        Signal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());
        assert!(worker.execute_step().is_continue());
        assert!(signal.is_notified());

        commands.send_system(SystemCommand::Shutdown).unwrap();
        worker.run();
    }

    #[test]
    fn a_full_command_batch_gives_the_primary_driver_no_wait_budget_and_skips_signal_wait() {
        let (commands, command_rx) = command_channel();
        let signal = Arc::new(Signal::default());
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::clone(&signal),
        );
        let waits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let _context = worker_context(&worker).io().context::<WaitRecordingContext>(&Arc::clone(&waits));
        for _ in 0..COMMANDS_PER_CYCLE {
            commands.send_spawn(enqueue(RemoteFutureFactory::new(|_, _| {}))).unwrap();
        }
        Signal::waker(&signal).wake_by_ref();
        assert!(signal.is_notified());

        assert!(worker.execute_step().is_continue());
        assert_eq!(*waits.lock().unwrap(), vec![Duration::ZERO]);
        assert!(signal.is_notified());

        commands.send_system(SystemCommand::Shutdown).unwrap();
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

        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let clock = worker_context(&worker).clock().clone();
        let mut timer = clock.delay(Duration::from_millis(1));
        let waker = Waker::from(Arc::into_std_arc(Arc::new(PanicWake)));
        assert!(Pin::new(&mut timer).poll(&mut std::task::Context::from_waker(&waker)).is_pending());
        // Forgetting a timer is legal and avoids a second panic from the poisoned timer lock.
        std::mem::forget(timer);
        thread::sleep(Duration::from_millis(2));
        commands.send_system(SystemCommand::Shutdown).unwrap();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run()));
        assert_eq!(*outcome.unwrap_err().downcast::<&str>().unwrap(), "final timer callback");
    }

    #[test]
    fn smoke_test() {
        async_test(async || {
            // Run worker, execute one remote task which awaits a nested task, then shut down.
            let (commands, command_rx) = command_channel();
            let worker_commands = commands.clone();
            let signal = Arc::new(Signal::default());
            let worker_signal = Arc::clone(&signal);

            let async_worker_thread = thread::spawn(move || {
                let blocking_worker = test_blocking_worker();

                let mut worker = test_worker(
                    (worker_commands, command_rx),
                    (Arc::clone(&blocking_worker.0), blocking_worker.1.clone()),
                    InactiveClock::new_isolated(),
                    worker_signal,
                );
                worker.run();

                assert!(blocking_worker.0.is_shutting_down());
            });

            // Signals that the task waiting on the nested task has completed.
            let (outer_completed_tx, outer_completed_rx) = Event::boxed();

            // Signals that the nested task has completed.
            let (inner_completed_tx, inner_completed_rx) = Event::boxed();

            commands
                .send_spawn(enqueue(RemoteFutureFactory::new({
                    move |_: Context, tasks: &TaskSet| {
                        tasks.add_test(async move {
                            inner_completed_tx.send(());
                        });
                        tasks.add_test(async move {
                            inner_completed_rx.await.unwrap();
                            outer_completed_tx.send(());
                        });
                    }
                })))
                .unwrap();
            Signal::waker(&signal).wake_by_ref();

            // Wait for the command to complete.
            outer_completed_rx.await.unwrap();

            // Shut down after the test body completes.
            commands.send_system(SystemCommand::Shutdown).unwrap();
            Signal::waker(&signal).wake_by_ref();

            // Join the worker so this test reports shutdown errors.
            async_worker_thread.join().unwrap();
        });
    }

    #[test]
    fn worker_setup_can_register_new_tasks() {
        async_test(async || {
            // Observe both tasks.
            let (initial_completed_tx, initial_completed_rx) = Event::boxed();
            let (spawned_completed_tx, spawned_completed_rx) = Event::boxed();

            let (commands, command_rx) = command_channel();
            let worker_commands = commands.clone();
            let signal = Arc::new(Signal::default());
            let worker_signal = Arc::clone(&signal);
            let worker_thread = thread::spawn(move || {
                let mut worker = test_worker(
                    (worker_commands, command_rx),
                    test_blocking_worker(),
                    InactiveClock::new_isolated(),
                    worker_signal,
                );
                worker.tasks.add_test(async move { initial_completed_tx.send(()) });

                worker.run();
            });

            commands
                .send_spawn(enqueue(RemoteFutureFactory::new({
                    move |_: Context, tasks: &TaskSet| {
                        tasks.add_test(async move {
                            spawned_completed_tx.send(());
                        });
                    }
                })))
                .unwrap();
            Signal::waker(&signal).wake_by_ref();

            spawned_completed_rx.await.unwrap();
            initial_completed_rx.into_value().unwrap();

            // Shut down after both tasks complete.
            commands.send_system(SystemCommand::Shutdown).unwrap();
            Signal::waker(&signal).wake_by_ref();

            // Join the worker so this test reports shutdown errors.
            worker_thread.join().unwrap();
        });
    }

    #[test]
    fn task_after_shutdown_is_ignored() {
        // The asynchronous submission path can enqueue a new-task command after
        // shutdown starts.
        //
        // The worker drops these tasks so their remote join handles report
        // shutdown and release all task resources.

        let (commands, command_rx) = command_channel();
        let worker_commands = commands.clone();
        let signal = Arc::new(Signal::default());
        let worker_signal = Arc::clone(&signal);

        let async_worker_thread = thread::spawn(move || {
            let mut worker = test_worker(
                (worker_commands, command_rx),
                test_blocking_worker(),
                InactiveClock::new_isolated(),
                worker_signal,
            );
            worker.run();
        });

        commands.send_system(SystemCommand::Shutdown).unwrap();
        Signal::waker(&signal).wake_by_ref();

        let (completed_tx, completed_rx) = Event::boxed();

        // The channel can already be closed because the worker is shutting down.
        let _ = commands.send_spawn(enqueue(RemoteFutureFactory::new({
            move |_: Context, tasks: &TaskSet| {
                tasks.add_test(async move {
                    completed_tx.send(14);
                });
            }
        })));

        // This waits for the worker to shut down.
        async_worker_thread.join().unwrap();

        let completed_result = completed_rx.into_value();
        assert!(matches!(completed_result, Err(IntoValueError::Disconnected)));
    }

    #[test]
    fn shutdown_command_cancels_registered_tasks_before_polling() {
        let (commands, command_rx) = command_channel();
        let invoked = Arc::new(AtomicBool::new(false));
        let invoked_by_task = Arc::clone(&invoked);

        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );

        worker.tasks.add_test(async move {
            invoked_by_task.store(true, Ordering::Relaxed);
        });
        commands.send_system(SystemCommand::Shutdown).unwrap();

        worker.run();

        assert!(!invoked.load(Ordering::Relaxed));
    }

    #[test]
    fn same_worker_stop_skips_a_later_initialized_task() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands, command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let control = worker_context(&worker).control().clone();
        let invoked = Arc::new(AtomicBool::new(false));
        let invoked_by_task = Arc::clone(&invoked);
        worker.tasks.add_test(async move {
            control.request_stop();
        });
        worker.tasks.add_test(async move {
            invoked_by_task.store(true, Ordering::Release);
        });
        let executor = worker_executor_mut(&mut worker);

        assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
        assert!(!invoked.load(Ordering::Acquire));
    }

    #[test]
    fn repeated_shutdown_cleanup_is_idempotent() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands, command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
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

        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let invoked = Arc::new(AtomicBool::new(false));
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let drop_signal = DropSignal(dropped_tx);
        commands
            .send_system(SystemCommand::AccessIo {
                access: QueuedIoAccess::new(Box::new({
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
        let (commands, command_rx) = command_channel();
        let invoked = Arc::new(AtomicUsize::new(0));
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let shutdown = worker.shutdown.clone();

        for index in 0..COMMANDS_PER_CYCLE {
            let invoked = Arc::clone(&invoked);
            let shutdown = shutdown.clone();
            commands
                .send_spawn(enqueue(RemoteFutureFactory::new(move |_, tasks| {
                    if index + 1 == COMMANDS_PER_CYCLE {
                        shutdown.begin(ShutdownCause::default());
                    }
                    tasks.add_test(async move {
                        invoked.fetch_add(1, Ordering::Relaxed);
                    });
                })))
                .unwrap();
        }

        worker.run();

        assert_eq!(invoked.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn shutdown_published_during_registration_skips_later_user_factories() {
        let (commands, command_rx) = command_channel();
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            test_blocking_worker(),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );
        let shutdown = worker.shutdown.clone();
        commands
            .send_spawn(enqueue(RemoteFutureFactory::new(move |_, _| {
                shutdown.begin(ShutdownCause::default());
            })))
            .unwrap();
        let invoked = Arc::new(AtomicBool::new(false));
        let join = worker_context(&worker).dispatcher().spawn({
            let invoked = Arc::clone(&invoked);
            move |_| {
                invoked.store(true, Ordering::Relaxed);
                std::future::ready(())
            }
        });

        assert!(worker.execute_step().is_break());

        assert!(!invoked.load(Ordering::Relaxed));
        let mut join = std::pin::pin!(join);
        let std::task::Poll::Ready(Err(error)) = join.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop())) else {
            panic!("task registered during shutdown must be cancelled");
        };
        assert!(error.is_shutdown());
    }

    #[test]
    fn dropping_worker_without_run_cleans_up_safely() {
        let commands = command_channel();
        let blocking_worker = test_blocking_worker();
        let worker = test_worker(
            commands,
            (Arc::clone(&blocking_worker.0), blocking_worker.1.clone()),
            InactiveClock::new_isolated(),
            Arc::new(Signal::default()),
        );

        drop(worker);

        assert!(blocking_worker.0.is_shutting_down());
    }

    #[test]
    fn explicit_executor_shutdown_completes_worker_shutdown() {
        let (commands, command_rx) = command_channel();
        let blocking_worker = test_blocking_worker();
        let signal = Arc::new(Signal::default());
        let mut worker = test_worker(
            (commands.clone(), command_rx),
            (Arc::clone(&blocking_worker.0), blocking_worker.1.clone()),
            InactiveClock::new_isolated(),
            Arc::clone(&signal),
        );
        Signal::waker(&signal).wake_by_ref();
        assert!(worker.execute_step().is_continue());
        worker_executor(&worker).shutdown().trigger(ShutdownCause::default());

        let outcome = worker.execute_step();

        assert!(outcome.is_break());
        assert!(worker_is_shutdown(&worker));
        assert!(blocking_worker.0.is_shutting_down());
        assert!(worker.process_system_commands().is_none());
        assert!(!worker.process_spawn_commands());
        drop(commands);
    }
}
