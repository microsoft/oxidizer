// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::OnceCell;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::{Duration, Instant};

use arty_executor::{CycleOutcome, Executor, TaskSet};
use performables::arc::Arc;
use performables::sync::channel;
use tick::Clock;
use tick::runtime::{ClockDriver, InactiveClock};

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::worker::protocol::AsyncWorkerCommand;
use crate::runtime::worker::signal::WorkerSignal;
use crate::task::Builtins;
use crate::task::local::LocalTaskScope;

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
/// 3. Shutting down - the worker has received a command to shut down and is in the process of doing
///    so. It may need to remain in this state for a while until non-cancelable work completes and
///    resources are released (e.g. it may need to wait for some timeouts to occur or even for other
///    threads to drop their interest in resources owned by the worker). During shutdown, new work
///    is not accepted and enqueued tasks are immediately discarded.
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
pub(in crate::runtime) struct AsyncWorker<TS = Builtins>
where
    TS: Clone + 'static,
{
    // Exclusively used by top level worker logic, never via reentrant task logic.
    // We drop this when we receive the shutdown signal, to ensure that any tasks that
    // happen to be enqueued after shutdown starts are discarded when an attempt is
    // made to send them.
    command_rx: Option<channel::Receiver<AsyncWorkerCommand<TS>>>,

    // Thread state initialization may require tasks to be executed, so we store the thread state in
    // a once cell. Before it's filled, we can only process tasks that don't depend on thread state.
    // We set this to None when we start shutdown - no more tasks can be enqueued after that and
    // dropping the thread state may be critical for the shutdown process to complete (the thread
    // state may hold resources that need to be released).
    thread_state: Option<Rc<OnceCell<TS>>>,

    // Becomes None during the graceful shutdown process. If this is not None in drop() then we
    // panic because we failed to follow the proper shutdown process.
    executor: Option<Executor>,

    // Connected to the `executor`. Does not participate in state management (just starts
    // to panic if an attempt to use it is made after executor shutdown starts).
    tasks: TaskSet,

    // We keep a reference to the blocking worker so that we can send it a shutdown signal.
    blocking_worker: Arc<BlockingWorker>,

    // This drives moves the timers registered with the clock forward.
    clock_driver: ClockDriver,

    signal: Arc<WorkerSignal>,
    _local_scope: LocalTaskScope,

    _single_threaded: PhantomData<*const ()>,
}

impl<TS> AsyncWorker<TS>
where
    TS: Clone + 'static,
{
    /// # Safety
    ///
    /// You must call `run()` before dropping the instance to ensure that the proper shutdown
    /// process is executed. Dropping the worker without first going through `run()` and the proper
    /// shutdown process will panic.
    pub(in crate::runtime) unsafe fn new<TSFF, TSF>(
        command_rx: channel::Receiver<AsyncWorkerCommand<TS>>,
        thread_state_constructor: TSFF,
        blocking_worker: Arc<BlockingWorker>,
        clock: InactiveClock,
        signal: Arc<WorkerSignal>,
        thread_state_constructed_tx: channel::Sender<()>,
    ) -> Self
    where
        TSFF: FnOnce(TaskSet, Clock) -> TSF + 'static,
        TSF: Future<Output = TS>,
    {
        let local_scope = LocalTaskScope::new();
        let (clock, clock_driver) = clock.activate();

        // SAFETY: We must go through the proper shutdown process before dropping this.
        // We guarantee this via our own safety requirement (it happens in `run()`). We know that
        // the caller has the chance to fulfill their safety guarantee because none of the code
        // between this point and end of the current function can panic.
        let executor = unsafe { Executor::builder().owner_waker(WorkerSignal::waker(&signal)).build() };
        let tasks = executor.tasks();

        let thread_state = Rc::new(OnceCell::new());

        tasks.add({
            let tasks = tasks.clone();
            let thread_state = Rc::clone(&thread_state);

            async move {
                let ts = thread_state_constructor(tasks, clock).await;
                thread_state
                    .set(ts)
                    .map_err(|__ts| ())
                    .expect("thread state initialized multiple times");
                _ = thread_state_constructed_tx.send_sync(());
            }
        });

        Self {
            command_rx: Some(command_rx),
            thread_state: Some(thread_state),
            executor: Some(executor),
            tasks,
            clock_driver,
            blocking_worker,
            signal,
            _local_scope: local_scope,
            _single_threaded: PhantomData,
        }
    }

    /// Worker thread entry point, invoked by bootstrap after startup coordination completes.
    /// The configuration-facing `RuntimeBuilder` delegates construction to bootstrap.
    /// After this method returns, the thread will end.
    pub(in crate::runtime) fn run(mut self) {
        self.execute_phase();
    }

    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn execute_phase(&mut self) {
        loop {
            let batch_exhausted = self.process_commands();

            let cycle_outcome = self
                .executor
                .as_ref()
                .expect("executor is not dropped until execute phase is finished")
                .execute_cycle();

            if matches!(cycle_outcome, CycleOutcome::Shutdown) {
                // No task wakers remain; retire their storage before the final timer callbacks.
                self.executor = None;
            }

            // Advances the timers registered with the clock.
            _ = self.clock_driver.advance_timers(Instant::now());

            match cycle_outcome {
                CycleOutcome::Continue => {}
                // Retain the donor's timer cadence, including externally controlled clocks.
                // A notification arriving before this wait is retained by WorkerSignal.
                CycleOutcome::Suspend if !batch_exhausted => self.signal.wait(SUSPEND_SLEEP_DURATION),
                CycleOutcome::Suspend => {}
                // This is the only way to exit the loop, guaranteeing safe shutdown of the
                // executor. The storage has already been retired before timer callbacks run.
                CycleOutcome::Shutdown => break,
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing requires a fine level of control over what is in the channel, which is too bothersome just for mutation testing.
    fn process_commands(&mut self) -> bool {
        let Some(thread_state) = self.thread_state.as_ref() else {
            debug_assert!(self.command_rx.is_none());

            // We are shutting down and have dropped the thread state.
            // The command channel is also closed, so no more commands can be received.
            return false;
        };

        let Some(thread_state) = thread_state.get() else {
            // The initialization hasn't fully finished yet, we cannot process commands at this point. Because the initialization
            // task doesn't get access to the dispatcher, we are sure it can finish without commands needing to be processed (unless
            // the user blocks on a task spawned on the main scheduler, but that's clearly their issue and is documented).
            return false;
        };

        let command_rx = self
            .command_rx
            .as_ref()
            .expect("thread state and command channel are dropped together");

        for _ in 0..COMMANDS_PER_CYCLE {
            match command_rx.try_recv() {
                Ok(AsyncWorkerCommand::EnqueueTask { future_factory }) => {
                    // This will never be reached during shutdown because as soon as shutdown
                    // starts, we close the command channel.
                    future_factory(thread_state.clone(), &self.tasks);
                }
                Ok(AsyncWorkerCommand::Shutdown) => {
                    self.begin_shutdown();

                    // Shutdown closes the command channel - no more commands can be received.
                    return false;
                }
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
        // Destructors of cancelled tasks may try to submit local cleanup work.
        // Close admission before executor cancellation borrows its task storage.
        LocalTaskScope::close();
        // Drop the thread state, as it may hold references to resources that
        // block executor shutdown (via various futures, waiters, etc).
        self.thread_state = None;

        // The command channel is henceforth closed. We will not receive any more commands.
        self.command_rx = None;

        self.blocking_worker.shutdown();

        // The executor can now start its own shutdown process. For us this does not change
        // anything - we are still required to keep executing executor cycles until it decides to
        // stop. This merely starts the executor shutdown.
        self.executor
            .as_mut()
            .expect("executor is not dropped until execute phase is finished")
            .begin_shutdown();
    }
}

impl<TS> Drop for AsyncWorker<TS>
where
    TS: Clone + 'static,
{
    #[cfg_attr(coverage_nightly, coverage(off))] // Only enforces the executor shutdown invariant.
    #[cfg_attr(test, mutants::skip)] // Safety on drop is validated on multiple levels, so might panic even if this is mutated away.
    fn drop(&mut self) {
        if self.executor.is_some() {
            // Unwinding cannot release storage still referenced by escaped task wakers.
            // Timer callbacks can poison clock state, so restarting shutdown is not safe here.
            std::process::abort();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    #[cfg(not(miri))]
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[cfg(not(miri))]
    use std::task::{Wake, Waker};
    use std::thread;

    use events_once::{Event, IntoValueError};
    use observed::Sink;
    use testing_aids::{async_test, execute_or_terminate_process};
    use tick::ClockControl;

    use super::*;
    use crate::runtime::blocking_worker::BlockingPool;
    use crate::runtime::blocking_worker::blocking_worker_tests::is_blocking_worker_shutting_down;
    use crate::task::local::{LocalTaskBinding, LocalTaskScheduler};

    #[derive(Clone, Debug)]
    struct TestTaskContext {
        local_task_scheduler: LocalTaskScheduler,
    }

    impl TestTaskContext {
        #[cfg_attr(test, mutants::skip)]
        fn new(tasks: TaskSet) -> Self {
            Self {
                local_task_scheduler: LocalTaskBinding::new(tasks, Sink::noop(), Arc::new(std::sync::atomic::AtomicBool::new(false)))
                    .local_scheduler()
                    .unwrap(),
            }
        }
    }

    fn replenishing_command(sender: channel::Sender<AsyncWorkerCommand<()>>, processed: Arc<AtomicUsize>) -> AsyncWorkerCommand<()> {
        AsyncWorkerCommand::EnqueueTask {
            future_factory: Box::new(move |(), _| {
                processed.fetch_add(1, Ordering::Relaxed);
                sender
                    .send_sync(replenishing_command(sender.clone(), Arc::clone(&processed)))
                    .unwrap();
            }),
        }
    }

    #[test]
    fn a_never_empty_command_queue_allows_registered_tasks_and_timers_to_progress() {
        execute_or_terminate_process(|| {
            let (command_tx, command_rx) = channel::unbounded();
            let (ready_tx, ready_rx) = channel::unbounded();
            let (proceed, progress) = Event::boxed();
            let control = ClockControl::new();
            let task_done = Arc::new(AtomicBool::new(false));
            let timer_done = Arc::new(AtomicBool::new(false));
            let processed = Arc::new(AtomicUsize::new(0));
            let task_observer = Arc::clone(&task_done);
            let timer_observer = Arc::clone(&timer_done);
            let shutdown = command_tx.clone();

            // SAFETY: run drives this worker to complete shutdown before it is dropped.
            let worker = unsafe {
                AsyncWorker::new(
                    command_rx,
                    async move |tasks, clock| {
                        drop(tasks.add(async move {
                            progress.await.unwrap();
                            task_observer.store(true, Ordering::Relaxed);
                        }));
                        drop(tasks.add(async move {
                            clock.delay(Duration::from_secs(1)).await;
                            timer_observer.store(true, Ordering::Relaxed);
                            shutdown.send_sync(AsyncWorkerCommand::Shutdown).unwrap();
                        }));
                    },
                    BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                    control.clone().into(),
                    Arc::new(WorkerSignal::default()),
                    ready_tx,
                )
            };
            let executor = worker.executor.as_ref().unwrap();
            let _ = executor.execute_cycle();
            ready_rx.recv_sync().unwrap();
            let _ = executor.execute_cycle();
            assert!(!task_done.load(Ordering::Relaxed));
            assert!(!timer_done.load(Ordering::Relaxed));

            command_tx
                .send_sync(replenishing_command(command_tx.clone(), Arc::clone(&processed)))
                .unwrap();
            proceed.send(());
            control.advance(Duration::from_secs(1));
            worker.run();

            assert!(task_done.load(Ordering::Relaxed));
            assert!(timer_done.load(Ordering::Relaxed));
            assert!((COMMANDS_PER_CYCLE..=2 * COMMANDS_PER_CYCLE + 1).contains(&processed.load(Ordering::Relaxed)));
        });
    }

    #[cfg(not(miri))]
    #[test]
    fn final_timer_panic_after_completed_shutdown_does_not_trigger_the_lifecycle_guard() {
        struct PanicWake;

        impl Wake for PanicWake {
            fn wake(self: std::sync::Arc<Self>) {
                panic!("final timer callback");
            }
        }

        let (command_tx, command_rx) = channel::unbounded();
        let (ready_tx, ready_rx) = channel::unbounded();
        let (clock_tx, clock_rx) = channel::unbounded();
        // SAFETY: run reaches Shutdown and retires the executor before the final timer callback.
        let worker = unsafe {
            AsyncWorker::new(
                command_rx,
                async move |_, clock| clock_tx.send_sync(clock).unwrap(),
                BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                InactiveClock::default(),
                Arc::new(WorkerSignal::default()),
                ready_tx,
            )
        };
        let _ = worker.executor.as_ref().unwrap().execute_cycle();
        ready_rx.recv_sync().unwrap();
        let _ = worker.executor.as_ref().unwrap().execute_cycle();
        let clock = clock_rx.recv_sync().unwrap();
        let mut timer = clock.delay(Duration::from_millis(1));
        let waker = Waker::from(Arc::into_std_arc(Arc::new(PanicWake)));
        assert!(Pin::new(&mut timer).poll(&mut std::task::Context::from_waker(&waker)).is_pending());
        // Forgetting a timer is legal and avoids a second panic from the poisoned timer lock.
        std::mem::forget(timer);
        thread::sleep(Duration::from_millis(2));
        command_tx.send_sync(AsyncWorkerCommand::Shutdown).unwrap();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run()));
        assert_eq!(*outcome.unwrap_err().downcast::<&str>().unwrap(), "final timer callback");
    }

    #[test]
    fn smoke_test() {
        async_test(async || {
            // Run worker, execute one remote task which executes and awaits one local task, shut down.
            let (command_tx, command_rx) = channel::unbounded();

            let async_worker_thread = thread::spawn(move || {
                let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());

                let signal = Arc::new(WorkerSignal::default());

                // SAFETY: We are required to call `run()` and must not drop the worker before that.
                // Okay. We are calling `run()` right now, so we are all good on that front.
                let worker = unsafe {
                    AsyncWorker::new(
                        command_rx,
                        async move |tasks, _| TestTaskContext::new(tasks),
                        Arc::clone(&blocking_worker),
                        InactiveClock::default(),
                        signal,
                        channel::unbounded().0,
                    )
                };
                execute_or_terminate_process(move || worker.run());

                // The async worker owns the blocking worker in our current implementation,
                // so we verify that we properly terminated it together with the async worker.
                assert!(is_blocking_worker_shutting_down(&blocking_worker));
            });

            // We set this to signal that the outer task (the remote one) has successfully completed.
            let (outer_completed_tx, outer_completed_rx) = Event::boxed();

            // We set this to signal that the inner task (the local one) has successfully completed.
            let (inner_completed_tx, inner_completed_rx) = Event::boxed();

            command_tx
                .send_sync(AsyncWorkerCommand::EnqueueTask {
                    future_factory: Box::new({
                        move |cx: TestTaskContext, tasks: &TaskSet| {
                            drop(tasks.add(async move {
                                cx.local_task_scheduler
                                    .spawn(async move || {
                                        inner_completed_tx.send(());
                                    })
                                    .await
                                    .unwrap();

                                outer_completed_tx.send(());
                            }));
                        }
                    }),
                })
                .unwrap();

            // Command sent! Now wait for something to happen.
            outer_completed_rx.await.unwrap();
            inner_completed_rx.into_value().unwrap();

            // Test body completed. Now let's shut it down.
            command_tx.send_sync(AsyncWorkerCommand::Shutdown).unwrap();

            // Wait for the worker to finish. It is harmless to continue immediately but we wait
            // just in case some errors occurred during shutdown - in which case we want to panic here.
            async_worker_thread.join().unwrap();
        });
    }

    #[test]
    fn thread_state_constructor_creates_new_tasks() {
        async_test(async || {
            // Executing async logic and creating new tasks from within this logic
            // is perfectly legal during creation of the thread state.

            // To verify tasks get executed.
            let (initial_completed_tx, initial_completed_rx) = Event::boxed();
            let (spawned_completed_tx, spawned_completed_rx) = Event::boxed();
            let (constructed_tx, constructed_rx) = channel::unbounded();

            let (command_tx, command_rx) = channel::unbounded();
            let worker_thread = thread::spawn(move || {
                let signal = Arc::new(WorkerSignal::default());

                // SAFETY: We are required to call `run()` and must not drop the worker before that.
                // Okay. We are calling `run()` right now, so we are all good on that front.
                let worker = unsafe {
                    AsyncWorker::new(
                        command_rx,
                        async move |tasks, _| {
                            drop(tasks.add(async move { initial_completed_tx.send(()) }));
                            TestTaskContext::new(tasks)
                        },
                        BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                        InactiveClock::default(),
                        signal,
                        constructed_tx,
                    )
                };

                execute_or_terminate_process(move || worker.run());
            });

            command_tx
                .send_sync(AsyncWorkerCommand::EnqueueTask {
                    future_factory: Box::new({
                        move |_: TestTaskContext, tasks: &TaskSet| {
                            drop(tasks.add(async move {
                                spawned_completed_tx.send(());
                            }));
                        }
                    }),
                })
                .unwrap();

            constructed_rx.recv_sync().unwrap();
            spawned_completed_rx.await.unwrap();
            initial_completed_rx.into_value().unwrap();

            // Tasks worked fine. Now let's shut it down.
            command_tx.send_sync(AsyncWorkerCommand::Shutdown).unwrap();

            // Wait for the worker to finish. It is harmless to continue immediately but we wait
            // just in case some errors occurred during shutdown - in which case we want to panic here.
            worker_thread.join().unwrap();
        });
    }

    #[test]
    fn task_after_shutdown_is_ignored() {
        // The executor will panic if you try to schedule a task after shutdown. However, it is
        // entirely possible that we have some "new task" commands in the async worker queue even
        // after starting the shutdown process, due to the async nature of task scheduling.
        //
        // While we cannot honor these commands, we must still do something and ensure that all
        // resources are properly managed. What we do is simply drop the tasks on the floor and
        // make their remote join handles report shutdown.

        let (command_tx, command_rx) = channel::unbounded();

        let async_worker_thread = thread::spawn(move || {
            let signal = Arc::new(WorkerSignal::default());

            // SAFETY: We are required to call `run()` and must not drop the worker before that.
            // Okay. We are calling `run()` right now, so we are all good on that front.
            let worker = unsafe {
                AsyncWorker::new(
                    command_rx,
                    async move |tasks, _| TestTaskContext::new(tasks),
                    BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
                    InactiveClock::default(),
                    signal,
                    channel::unbounded().0,
                )
            };
            execute_or_terminate_process(move || worker.run());
        });

        command_tx.send_sync(AsyncWorkerCommand::Shutdown).unwrap();

        let (completed_tx, completed_rx) = Event::boxed();

        // We ignore the result because the worker is shutting down and the channel might be closed already.
        let _ = command_tx.send_sync(AsyncWorkerCommand::EnqueueTask {
            future_factory: Box::new({
                move |_: TestTaskContext, tasks: &TaskSet| {
                    drop(tasks.add(async move {
                        completed_tx.send(14);
                    }));
                }
            }),
        });

        // This waits for the worker to shut down.
        execute_or_terminate_process(move || async_worker_thread.join()).unwrap();

        let completed_result = completed_rx.into_value();
        assert!(matches!(completed_result, Err(IntoValueError::Disconnected)));
    }

    #[test]
    fn shutdown_before_initialization_discards_the_constructor() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let initialized = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&initialized);
        let (_commands, receiver) = channel::unbounded();
        let blocking_worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());
        // SAFETY: run completes the executor's shutdown before the worker is dropped.
        let mut worker = unsafe {
            AsyncWorker::new(
                receiver,
                async move |tasks, _| {
                    observed.store(true, Ordering::Relaxed);
                    TestTaskContext::new(tasks)
                },
                Arc::clone(&blocking_worker),
                InactiveClock::default(),
                Arc::new(WorkerSignal::default()),
                channel::unbounded().0,
            )
        };

        worker.process_commands();
        worker.begin_shutdown();
        worker.process_commands();
        worker.run();

        assert!(!initialized.load(Ordering::Relaxed));
        assert!(is_blocking_worker_shutting_down(&blocking_worker));
    }
}
