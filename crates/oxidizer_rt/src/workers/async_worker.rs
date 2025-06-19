// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::cell::OnceCell;
use std::fmt::Debug;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};
use std::{fmt, thread};

use negative_impl::negative_impl;
use oxidizer_time::Clock;
use oxidizer_time::runtime::{ClockDriver, InactiveClock};

use crate::{
    AbstractAsyncTaskExecutor, CycleResult, RuntimeThreadState, SpawnQueue, SystemWorker,
    WakerWaiterFacade,
};

/// Placeholder until we have a real non-sleeping implementation.
/// We sleep when nothing to do, to avoid just having a busy loop burning CPU.
const SUSPEND_SLEEP_DURATION: Duration = Duration::from_millis(1);

/// The async worker has exclusive use of a worker thread of the runtime, one per processor
/// (if not quota-limited). The worker uses the thread to execute async tasks that are
/// typically I/O bound, as well as for processing I/O completion notifications arriving in response
/// to I/O operations (typically but not always for operations started from the same thread).
///
/// The type represents the internal state of the worker. This state is connected to other objects
/// through messaging channels that allow components of the runtime to deliver work and signals.
///
/// # Ownership
///
/// This type is exclusively owned by the thread entrypoint.
///
/// # Lifecycle
///
/// The worker lifecycle consists of multiple stages:
///
/// 1. Startup - a [`RuntimeBuilder`][crate::RuntimeBuilder] has created the thread and the worker
///    and the worker has started initializing itself. This may involve some back and forth
///    coordination between the two until the worker is satisfied that it can start operating.
/// 2. Running - the worker is processing tasks and I/O completions and is waiting for commands.
/// 3. Shutting down - the worker has received a command to shut down and is in the process of doing
///    so. It may need to remain in this state for a while until non-cancelable work completes and
///    resources are released (e.g. it may need to wait for some timeouts to occur or even for other
///    threads to drop their interest in resources owned by the worker). During shutdown, new work
///    is not accepted and enqueued tasks are immediately discarded.
/// 4. Dead - the worker has completed its shutdown process and the thread has terminated.
///
/// Once in the Running state, workers keep running until they receive a stop command (i.e. until
/// something calls [`stop()`][crate::abstractions::StopRuntime] on one of the runtime-related
/// context objects).
///
/// # Thread safety
///
/// This type is single-threaded - it only exists on the designated worker thread.
#[derive(Debug)]
pub struct AsyncWorker<E, TS>
where
    E: AbstractAsyncTaskExecutor,
    TS: RuntimeThreadState,
{
    // Exclusively used by top level worker logic, never via reentrant task logic.
    command_rx: mpsc::Receiver<AsyncWorkerCommand<TS>>,

    // Tasks that have been enqueued on the worker but have not yet been handed over to the
    // executor. This includes both local tasks (queued by other tasks on this thread) and remote
    // tasks (queued by other workers on other threads). By the time the task lands here, we have
    // already erased the difference between local and remote tasks and treat them the same - all
    // tasks are local tasks as far as the worker is concerned, they just got to this point via
    // different paths. Once a task is added to the queue, we have an obligation to process it.
    spawn_queue: Rc<SpawnQueue>,

    // Thread state initialization may require tasks to be executed, so we store the thread state in
    // a once cell. Before it's filled, we can only process tasks that don't depend on thread state.
    thread_state: Rc<OnceCell<TS>>,

    // Becomes None during the graceful shutdown process. If this is not None in drop() then we
    // panic because we failed to follow the proper shutdown process.
    executor: Option<E>,

    // Once shutdown has started, we ignore remote requests to enqueue new tasks and discard them
    // immediately - the only thing we care about during shutdown is cleanup of resources.
    shutdown_started: bool,

    // We keep a reference to the system worker so that we can send it a shutdown signal.
    system_worker: Rc<SystemWorker>,

    // This drives moves the timers registered with the clock forward.
    clock_driver: ClockDriver,

    waker_waiter: WakerWaiterFacade,
}

impl<E, TS> AsyncWorker<E, TS>
where
    E: AbstractAsyncTaskExecutor,
    TS: RuntimeThreadState,
{
    /// # Safety
    ///
    /// You must call `run()` before dropping the instance to ensure that the proper shutdown
    /// process is executed. Dropping the worker without first going through `run()` and the proper
    /// shutdown process will panic.
    #[expect(clippy::too_many_arguments, reason = "this will be refactored later")]
    pub unsafe fn new<TSFF, TSF>(
        command_rx: mpsc::Receiver<AsyncWorkerCommand<TS>>,
        executor: E,
        thread_state_constructor: TSFF,
        system_worker: Rc<SystemWorker>,
        clock: InactiveClock,
        waker_waiter: WakerWaiterFacade,
        spawn_queue: Rc<SpawnQueue>,
        success_tx: oneshot::Sender<Result<(), TS::Error>>,
    ) -> Self
    where
        TSFF: FnOnce(Rc<SpawnQueue>, Clock) -> TSF + 'static,
        TSF: Future<Output = Result<TS, TS::Error>>,
    {
        let (clock, clock_driver) = clock.activate();

        let thread_state = Rc::new(OnceCell::new());

        let spawn_queue_clone = Rc::clone(&spawn_queue);
        let thread_state_clone = Rc::clone(&thread_state);

        spawn_queue.spawn_local(async move || {
            let ts = thread_state_constructor(spawn_queue_clone, clock).await;

            success_tx
                .send(match ts {
                    Ok(ts) => {
                        thread_state_clone
                            .set(ts)
                            .map_err(|__ts| ())
                            .expect("thread state initialized multiple times");

                        Ok(())
                    }
                    Err(err) => Err(err),
                })
                .expect("thread state initialization failed - receiver dropped");
        });

        Self {
            command_rx,
            spawn_queue,
            thread_state,
            executor: Some(executor),
            shutdown_started: false,
            clock_driver,
            system_worker,
            waker_waiter,
        }
    }

    /// Worker thread entrypoint. This is called by `RuntimeBuilder` when the startup stage has
    /// completed and the worker can now be used. After this method returns, the thread will end.
    pub fn run(mut self) {
        assert!(
            !self.shutdown_started,
            "worker started with shutdown already in progress - should be impossible without 2x run()"
        );

        self.execute_phase();

        // The execution phase has finished, so there is no reason to keep the executor around.
        // Dropping it here enables additional sanity checks in shutdown operation ordering.
        self.executor = None;

        self.io_shutdown_phase();
    }

    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn execute_phase(&mut self) {
        loop {
            match self.process_commands() {
                ProcessCommandsResult::ContinueWithNewTasks
                | ProcessCommandsResult::ContinueWithoutNewTasks => {
                    // In the future, we will differentiate how we execute the loop iteration based
                    // on whether (and which) commands were received. For now, we do not care.
                }
                ProcessCommandsResult::Shutdown => {
                    self.begin_shutdown();
                }
            }

            if !self.shutdown_started {
                // If shutdown has started we know that there cannot be any more tasks queued,
                // so we only need to check for tasks if we are not in shutdown. Indeed, the task
                // queue would be unhappy with us if we tried to dequeue from it during shutdown.
                self.accept_queued_tasks();
            }

            // We assert unwind safety because when a panic happens, we start the shutdown process,
            // which will drop all tasks and thereby minimize the chance of any data corruption in
            // there (although any shared data may still be in a corrupted state). Not perfect but
            // that is what you get if you panic.
            let cycle_result = catch_unwind(AssertUnwindSafe(|| {
                self.executor
                    .as_mut()
                    .expect("executor is not dropped until execute phase is finished")
                    .execute_cycle()
            }))
            .unwrap_or_else(|_| {
                // If task executions panics, we start shutdown. This immediately terminates
                // all tasks and begins the shutdown process. We try to go through the
                // normal process so that we have minimal harmful effects in tests and
                // avoid additional panics due to dirty shutdown assertions firing.
                self.begin_shutdown();

                // We do not bother doing anything with the value of the panic because it seems
                // panics are automatically printed to stderr, so there is not much more value
                // we can add here - the telemetry is visible to a stderr observer.

                // Immediately enter a new cycle to progress the shutdown.
                CycleResult::Continue
            });

            // Advances the timers registered with the clock.
            _ = self.clock_driver.advance_timers(Instant::now());

            // Placeholder logic, should be removed when we refactor the Waker to queue the task immediately.
            if self.spawn_queue.has_new_tasks() {
                // We have new tasks so we should not sleep - we should immediately start the next cycle.
                continue;
            }

            match cycle_result {
                // Placeholder logic, to be replaced with real logic when I/O driver is integrated.
                CycleResult::Continue => thread::yield_now(),
                CycleResult::Suspend => self.waker_waiter.wait(SUSPEND_SLEEP_DURATION),
                CycleResult::Shutdown => break,
            }
        }
    }

    fn process_commands(&self) -> ProcessCommandsResult {
        let Some(thread_state) = self.thread_state.get() else {
            // The initialization hasn't fully finished yet, we cannot process commands at this point. Because the initialization
            // task doesn't get access to the dispatcher, we are sure it can finish without commands needing to be processed (unless
            // the user blocks on a task spawned on the main scheduler, but that's clearly their issue and is documented).
            return ProcessCommandsResult::ContinueWithoutNewTasks;
        };

        let mut received_shutdown = false;
        let mut received_new_tasks = false;

        loop {
            match self.command_rx.try_recv() {
                Ok(AsyncWorkerCommand::EnqueueTask { future_factory }) => {
                    if self.shutdown_started || received_shutdown {
                        // We are in the process of shutting down or have just now received a
                        // shutdown command. We therefore discard the task and refuse to accept it.
                        continue;
                    }

                    received_new_tasks = true;
                    let thread_state_clone = thread_state.clone();
                    self.spawn_queue
                        .spawn_local(move || future_factory(thread_state_clone));
                }
                Ok(AsyncWorkerCommand::Shutdown) => {
                    received_shutdown = true;
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    // The runtime is self-referential - every worker knows how to send commands to
                    // every other worker -, so it should be impossible for the sender to drop first.
                    unreachable!("async worker command channel disconnected without shutdown");
                }
            }
        }

        if received_shutdown {
            ProcessCommandsResult::Shutdown
        } else if received_new_tasks {
            ProcessCommandsResult::ContinueWithNewTasks
        } else {
            ProcessCommandsResult::ContinueWithoutNewTasks
        }
    }

    #[cfg_attr(test, mutants::skip)] // If mutated, shutdown process will never finish - will hang.
    fn begin_shutdown(&mut self) {
        if self.shutdown_started {
            // This may be called in duplicate if we received multiple parallel shutdown commands.
            return;
        }

        self.system_worker.shutdown();

        // This prevents new local tasks from being created. Some "new task" commands may still
        // arrive in our command queue (and will be received + dropped). We do not care about those
        // because they are "partial", merely being a future factory that has not yet been used
        // to construct an AsyncTask implementation (which would carry resource management duties).
        self.shutdown_started = true;

        // We have an obligation to correctly process all tasks that are ever enqueued by the
        // worker. While the above prevents new tasks from arriving, there may already be tasks in
        // there. Pass them over to the executor so they go through the standard shutdown process.
        self.accept_queued_tasks();

        // The executor can now start its own shutdown process. For us this does not change
        // anything - we are still required to keep executing executor cycles until it decides to
        // stop. This merely starts the executor shutdown.
        self.executor
            .as_mut()
            .expect("executor is not dropped until execute phase is finished")
            .begin_shutdown();
    }

    /// Accepts all tasks from the task queue for processing by the executor.
    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn accept_queued_tasks(&mut self) {
        let executor = self
            .executor
            .as_mut()
            .expect("executor is not dropped until execute phase is finished");

        // SAFETY: We have an obligation not to drop these before processing, which is fulfilled
        // by the executor.
        unsafe {
            self.spawn_queue.drain(|tasks| {
                for task in tasks {
                    executor.enqueue(task);
                }
            });
        }
    }

    // This phase is when the executor has already been shut down but we may still need to wait
    // for I/O processing to complete. This is the final phase before the worker thread ends.
    #[cfg_attr(test, mutants::skip)]
    fn io_shutdown_phase(&mut self) {
        while !self.waker_waiter.is_inert() {
            self.waker_waiter.wait(SUSPEND_SLEEP_DURATION);
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ProcessCommandsResult {
    // At least one new task was received when processing commands.
    ContinueWithNewTasks,

    // No new tasks were received when processing commands.
    ContinueWithoutNewTasks,

    // We received a shutdown command - stop task execution and clean up everything ASAP.
    Shutdown,
}

impl<E, TS> Drop for AsyncWorker<E, TS>
where
    E: AbstractAsyncTaskExecutor,
    TS: RuntimeThreadState,
{
    fn drop(&mut self) {
        if thread::panicking() {
            // We skip the assertions if we are already panicking because a double panic more often
            // does not help anything and may even obscure the initial panic in test runs.
            return;
        }

        assert!(
            self.shutdown_started,
            "{} dropped without proper shutdown",
            type_name::<Self>()
        );

        // TODO: Once we have a proper I/O driver implementation, review whether we really need to
        // drop this stuff earlier or can just do it here to simplify the code. Right now this is
        // being overly conservative as the current implementation does not require early drop but
        // other potential implementations might (but unsure which one we end up with).
        assert!(
            self.executor.is_none(),
            "{} dropped without proper shutdown",
            type_name::<Self>()
        );
    }
}

#[negative_impl]
impl<E, TS> !Send for AsyncWorker<E, TS>
where
    E: AbstractAsyncTaskExecutor,
    TS: RuntimeThreadState,
{
}
#[negative_impl]
impl<E, TS> !Sync for AsyncWorker<E, TS>
where
    E: AbstractAsyncTaskExecutor,
    TS: RuntimeThreadState,
{
}

/// A future factory for a remote future scheduled from a different thread. The future factory
/// itself must be `Send` to deliver it to the thread where the task is to be scheduled but this
/// does not set any constraints on the future returned by the factory - it may be single-threaded.
///
/// The future factory is boxed up for transit between threads and has 'static to signal that it has
/// no dependency on the stack of any specific thread.
pub type BoxedRemoteFutureFactory<FgArg> =
    Box<dyn (FnOnce(FgArg) -> Pin<Box<dyn Future<Output = ()>>>) + Send + 'static>;

pub enum AsyncWorkerCommand<TS> {
    /// Enqueues a new task for execution on this worker, providing the factory function that will
    /// be used to create the future that becomes the body of the task.
    ///
    /// Note that these remotely enqueued tasks do not have an output type - it is the
    /// responsibility of the task itself to deliver any outputs to some waiting thread. In
    /// practice, this means there will be two layers of futures: an outer layer responsible for
    /// delivering the output, and an inner layer with the actual user code to execute.
    ///
    /// The task may end up never getting executed if the runtime is shut down before it gets to it.
    EnqueueTask {
        future_factory: BoxedRemoteFutureFactory<TS>,
    },

    /// Initiates the shutdown process. The worker will stop accepting new tasks and will discard
    /// any tasks that are enqueued after this command is received (e.g. because other threads do
    /// not yet know about the shutdown).
    ///
    /// It is fine to send this command multiple times - duplicates will be ignored.
    Shutdown,
}

impl<TS> fmt::Debug for AsyncWorkerCommand<TS> {
    #[cfg_attr(test, mutants::skip)] // We have no contract to test here - can return anything.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnqueueTask { .. } => write!(f, "EnqueueTask"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::Context;

    use futures::FutureExt;
    use futures::task::noop_waker;
    use mockall::Sequence;
    use oxidizer_testing::{TEST_TIMEOUT, execute_or_abandon, execute_or_terminate_process};

    use super::*;
    use crate::system_worker_tests::is_system_worker_shutting_down;
    use crate::{
        AsyncTaskExecutor, LocalTaskMeta, MockAsyncTaskExecutor, TestTaskContext, ThreadWaker,
        YieldFuture,
    };

    #[test]
    fn smoke_test() {
        // Run worker, execute one remote task which executes and awaits one local task, shut down.

        let (command_tx, command_rx) = mpsc::channel();

        let worker_thread = thread::spawn(move || {
            // SAFETY: We have to do proper shutdown of this, which is performed by the worker.
            let executor = unsafe { AsyncTaskExecutor::new(ThreadWaker::new().into()) };

            let system_worker = SystemWorker::new();

            // SAFETY: We are not allowed to drop this without going through the proper shutdown
            // process that first drains the queue and then explicitly shuts it down. We do this
            // upon `run()` processing the shutdown command. Therefore, we impose an obligation for
            // the caller to actually call `run()`.
            let spawn_queue = unsafe { SpawnQueue::new() };

            let (succes_tx, _success_rx) = oneshot::channel();

            // SAFETY: We are required to call `run()` and must not drop the worker before that.
            // Okay. We are calling `run()` right now, so we are all good on that front.
            let worker = unsafe {
                AsyncWorker::new(
                    command_rx,
                    executor,
                    async move |q, _| Ok(TestTaskContext::new(Rc::downgrade(&q))),
                    Rc::clone(&system_worker),
                    InactiveClock::default(),
                    ThreadWaker::new().into(),
                    spawn_queue,
                    succes_tx,
                )
            };
            execute_or_terminate_process(move || worker.run());
            assert!(is_system_worker_shutting_down(&system_worker));
        });

        // We set this to signal that the outer task (the remote one) has successfully completed.
        let (outer_completed_tx, outer_completed_rx) = oneshot::channel();

        // We set this to signal that the inner task (the local one) has successfully completed.
        let (inner_completed_tx, inner_completed_rx) = oneshot::channel();

        command_tx
            .send(AsyncWorkerCommand::EnqueueTask {
                future_factory: Box::new({
                    move |cx| {
                        Box::pin(async move {
                            cx.local_task_scheduler
                                .spawn_with_meta(LocalTaskMeta::default(), async move || {
                                    _ = inner_completed_tx.send(());
                                })
                                .await;

                            _ = outer_completed_tx.send(());
                        })
                    }
                }),
            })
            .unwrap();

        // Command sent! Now wait for something to happen.
        outer_completed_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        inner_completed_rx.try_recv().unwrap();

        // Tasks worked fine. Now let's shut it down.
        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

        // Wait for the worker to finish. It is harmless to continue immediately but we wait
        // just in case some errors occurred during shutdown - in which case we want to panic here.
        execute_or_abandon(move || worker_thread.join())
            .unwrap()
            .unwrap();
    }

    #[test]
    fn async_task_constructor_using_scheduler() {
        // Oneshot channels to verify tasks get executed.
        let (initial_completed_tx, initial_completed_rx) = oneshot::channel();
        let (spawned_completed_tx, spawned_completed_rx) = oneshot::channel();

        let (command_tx, command_rx) = mpsc::channel();
        let worker_thread = thread::spawn(move || {
            // SAFETY: We have to do proper shutdown of this, which is performed by the worker.
            let executor = unsafe { AsyncTaskExecutor::new(ThreadWaker::new().into()) };

            // SAFETY: We are not allowed to drop this without going through the proper shutdown
            // process that first drains the queue and then explicitly shuts it down. We do this
            // upon `run()` processing the shutdown command. Therefore, we impose an obligation for
            // the caller to actually call `run()`.
            let spawn_queue = unsafe { SpawnQueue::new() };

            let (succes_tx, _success_rx) = oneshot::channel();

            // SAFETY: We are required to call `run()` and must not drop the worker before that.
            // Okay. We are calling `run()` right now, so we are all good on that front.
            let worker = unsafe {
                AsyncWorker::new(
                    command_rx,
                    executor,
                    async move |queue, _| {
                        Ok({
                            drop(queue.spawn_local(async move || initial_completed_tx.send(())));
                            TestTaskContext::new(Rc::downgrade(&queue))
                        })
                    },
                    SystemWorker::new(),
                    InactiveClock::default(),
                    ThreadWaker::new().into(),
                    spawn_queue,
                    succes_tx,
                )
            };

            execute_or_terminate_process(move || worker.run());
        });

        command_tx
            .send(AsyncWorkerCommand::EnqueueTask {
                future_factory: Box::new({
                    move |_| {
                        Box::pin(async move {
                            _ = spawned_completed_tx.send(());
                        })
                    }
                }),
            })
            .unwrap();

        spawned_completed_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        initial_completed_rx.try_recv().unwrap();

        // Tasks worked fine. Now let's shut it down.
        command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();

        // Wait for the worker to finish. It is harmless to continue immediately but we wait
        // just in case some errors occurred during shutdown - in which case we want to panic here.
        execute_or_abandon(move || worker_thread.join())
            .unwrap()
            .unwrap();
    }

    #[test]
    fn task_after_shutdown_is_ignored() {
        // We start the shutdown process and then, while the shutdown process is still ongoing,
        // we send a command to start a new task. We expect that this task does NOT get executed.

        let (command_tx, command_rx) = mpsc::channel();

        // We control what the worker does by using a mock executor that does little except keep the
        // worker running and act as our spy to inform us whether a task is scheduled.
        let mut executor = MockAsyncTaskExecutor::new();

        let mut seq = Sequence::new();

        // Only one task should get enqueued - the thread state initialization task.
        executor
            .expect_enqueue()
            .once()
            .in_sequence(&mut seq)
            .returning(|mut task| {
                assert!(
                    task.poll_unpin(&mut Context::from_waker(&noop_waker()))
                        .is_ready()
                );

                task.as_mut().clear();

                assert!(task.is_inert());
            });

        // When the first cycle is executed, we know the executor is running and can send the
        // shutdown message. We expect the worker to process this message on its next iteration.
        executor
            .expect_execute_cycle()
            .once()
            .in_sequence(&mut seq)
            .returning({
                let command_tx = command_tx.clone();

                move || {
                    command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
                    CycleResult::Continue
                }
            });

        // This confirms that the worker has received the shutdown command.
        executor
            .expect_begin_shutdown()
            .once()
            .in_sequence(&mut seq)
            .return_const(());

        // Shutdown has started, now we can try enqueue a task.
        executor
            .expect_execute_cycle()
            .once()
            .in_sequence(&mut seq)
            .returning({
                let command_tx = command_tx;

                move || {
                    command_tx
                        .send(AsyncWorkerCommand::EnqueueTask {
                            future_factory: Box::new(move |_cx| {
                                Box::pin(async move {
                                    YieldFuture::new().await;
                                })
                            }),
                        })
                        .unwrap();
                    CycleResult::Continue
                }
            });

        // We expect that no calls to enqueue some task were made as a result of our command.
        // Now that we have started the next executor cycle, it is now safe to shut down the worker,
        // which we do here by signaling executor shutdown. We know that we can end here because
        // commands are processed before the executor cycle is executed, so if no task was enqueued
        // by this point, all is good.
        executor
            .expect_execute_cycle()
            .once()
            .in_sequence(&mut seq)
            .return_const(CycleResult::Shutdown);

        let worker_thread = thread::spawn(move || {
            // SAFETY: We are not allowed to drop this without going through the proper shutdown
            // process that first drains the queue and then explicitly shuts it down. We do this
            // upon `run()` processing the shutdown command. Therefore, we impose an obligation for
            // the caller to actually call `run()`.
            let spawn_queue = unsafe { SpawnQueue::new() };

            let (succes_tx, _success_rx) = oneshot::channel();

            // SAFETY: We are required to call `run()` and must not drop the worker before that.
            // Okay. We are calling `run()` right now, so we are all good on that front.
            let worker = unsafe {
                AsyncWorker::<_, TestTaskContext>::new(
                    command_rx,
                    executor,
                    async move |q, _| Ok(TestTaskContext::new(Rc::downgrade(&q))),
                    SystemWorker::new(),
                    InactiveClock::default(),
                    ThreadWaker::new().into(),
                    spawn_queue,
                    succes_tx,
                )
            };
            execute_or_terminate_process(move || worker.run());
        });

        // If something went wrong (e.g. a task really was queued), we expect this to panic
        // due to a "did not expect enqueue()" panic from the mock.
        execute_or_abandon(move || worker_thread.join())
            .unwrap()
            .unwrap();
    }

    #[test]
    fn task_before_shutdown_is_accepted() {
        // We schedule a new task on the same cycle as the shutdown command. We expect that the task
        // is accepted and handed over to the executor (which will eventually cancel it once it sees
        // the shutdown command but that is already executor internal logic).

        let (command_tx, command_rx) = mpsc::channel();

        // We control what the worker does by using a mock executor that does little except keep the
        // worker running and act as our spy to inform us whether a task is scheduled.
        let mut executor = MockAsyncTaskExecutor::new();

        let mut seq = Sequence::new();

        // First enqueued task is the thread state initialization one. Without that, no tasks spawned
        // outside of local scheduler will be enqueued.
        executor
            .expect_enqueue()
            .once()
            .in_sequence(&mut seq)
            .returning(|mut task| {
                // We just need to poll the task once as there are no await in the init implementation below.
                assert!(
                    task.poll_unpin(&mut Context::from_waker(&noop_waker()))
                        .is_ready()
                );

                task.as_mut().clear();

                assert!(task.is_inert());
            });

        // When the first cycle is executed, we know the executor is running and can send the task
        // spawn message and immediately thereafter the shutdown message. We expect the worker to
        // process both messages on the same iteration.
        executor
            .expect_execute_cycle()
            .once()
            .in_sequence(&mut seq)
            .returning({
                move || {
                    command_tx
                        .send(AsyncWorkerCommand::EnqueueTask {
                            future_factory: Box::new(move |_cx| {
                                Box::pin(async move {
                                    YieldFuture::new().await;
                                })
                            }),
                        })
                        .unwrap();

                    command_tx.send(AsyncWorkerCommand::Shutdown).unwrap();
                    CycleResult::Continue
                }
            });

        // Exactly one task should be enqueued during this test. Note that we need to process the
        // task here, according to the requirements defined by the AsyncTask trait.
        executor
            .expect_enqueue()
            .once()
            .in_sequence(&mut seq)
            .returning(|mut task| {
                // We do not care what the task itself does (and will not actually execute it).
                // All we want to know is that it got queued, at which point the worker has done
                // its duty, and we simply complement that with the proper task shutdown sequence.
                task.as_mut().clear();

                // In principle, a future implementation could choose not to be inert at this point,
                // in which case we would have to complicate our test logic to allow the task to
                // become inert at its own pace. This test implementation serves us well for now.
                assert!(task.is_inert());
            });

        // This confirms that the worker has received the shutdown command.
        executor
            .expect_begin_shutdown()
            .once()
            .in_sequence(&mut seq)
            .return_const(());

        // The executor immediately reports shutdown completed, nothing else to do here.
        executor
            .expect_execute_cycle()
            .once()
            .in_sequence(&mut seq)
            .return_const(CycleResult::Shutdown);

        let worker_thread = thread::spawn(move || {
            // SAFETY: We are not allowed to drop this without going through the proper shutdown
            // process that first drains the queue and then explicitly shuts it down. We do this
            // upon `run()` processing the shutdown command. Therefore, we impose an obligation for
            // the caller to actually call `run()`.
            let spawn_queue = unsafe { SpawnQueue::new() };

            let (succes_tx, _success_rx) = oneshot::channel();

            // SAFETY: We are required to call `run()` and must not drop the worker before that.
            // Okay. We are calling `run()` right now, so we are all good on that front.
            let worker = unsafe {
                AsyncWorker::<_, TestTaskContext>::new(
                    command_rx,
                    executor,
                    async move |q, _| Ok(TestTaskContext::new(Rc::downgrade(&q))),
                    SystemWorker::new(),
                    InactiveClock::default(),
                    ThreadWaker::new().into(),
                    spawn_queue,
                    succes_tx,
                )
            };
            execute_or_terminate_process(move || worker.run());
        });

        // If something went wrong (e.g. a task really was queued), we expect this to panic
        // due to a "did not expect enqueue()" panic from the mock.
        execute_or_abandon(move || worker_thread.join())
            .unwrap()
            .unwrap();
    }

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    #[cfg(not(miri))] // Dirty drop may indeed cause invalid execution - that's why we panic.
    fn panic_on_dirty_drop() {
        let (_, command_rx) = mpsc::channel();

        // SAFETY: We are not allowed to drop this without going through the proper shutdown
        // process that first drains the queue and then explicitly shuts it down. We do this
        // upon `run()` processing the shutdown command. Therefore, we impose an obligation for
        // the caller to actually call `run()`.
        let spawn_queue = unsafe { SpawnQueue::new() };

        let (succes_tx, _success_rx) = oneshot::channel();

        // SAFETY: We deliberately violate the safety contract by dropping the worker without
        // calling `run()` to allow for it to shut down itself properly. This should panic.
        let _ = unsafe {
            AsyncWorker::<_, TestTaskContext>::new(
                command_rx,
                MockAsyncTaskExecutor::new(),
                async move |q, _| Ok(TestTaskContext::new(Rc::downgrade(&q))),
                SystemWorker::new(),
                InactiveClock::default(),
                ThreadWaker::new().into(),
                spawn_queue,
                succes_tx,
            )
        };
    }
}