// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod arguments;
mod processors;

use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, mpsc};
use std::thread;

pub use arguments::*;
use isolated_domains::{Domain, create_domains};
use many_cpus::Processor;
use nonempty::NonEmpty;
use oxidizer_time::runtime::InactiveClock;
pub use processors::*;
use tracing::{Level, event};

use crate::{
    AsyncTaskExecutor, AsyncWorker, AsyncWorkerCommand, DispatcherClient, DispatcherCore, Runtime,
    SpawnQueue, SystemWorker, ThreadWaiter, WakerFacade, WakerWaiterFacade, non_blocking_thread,
};

/// Collects the necessary data from the caller and the environment to build and start an instance
/// of the Oxidizer Runtime.
#[derive(Debug)]
pub struct RuntimeBuilder<TS = BasicThreadState>
where
    TS: RuntimeThreadState,
{
    processor_config: ResourceQuota,
    shared_state: TS::SharedState,
    clock: InactiveClock,
}

impl<TS> RuntimeBuilder<TS>
where
    TS: RuntimeThreadState,
{
    /// Sets the processor configuration for the Runtime to use.
    #[must_use]
    pub const fn with_resource_quota(mut self, config: ResourceQuota) -> Self {
        self.processor_config = config;
        self
    }

    /// Sets the clock to be used by the runtime.
    ///
    /// The [`InactiveClock`] represents a handle to a clock that's not yet active.
    /// It will be cloned to each async worker thread and activated on that thread.
    ///
    /// # Examples
    ///
    /// ### Explicitly setting the clock
    ///
    /// ```
    /// use oxidizer_rt::{RuntimeBuilder, BasicThreadState};
    /// use oxidizer_time::runtime::InactiveClock;
    ///
    /// let clock = InactiveClock::default();
    /// let runtime = RuntimeBuilder::new::<BasicThreadState>().with_clock(clock).build();
    /// ```
    ///
    /// ### Using the fake clock
    ///
    /// The [`oxidizer_time`] exposes the `ClockControl` type that allows you to
    /// control the flow of time in test scenarios. The `ClockControl` can be used in this method too.
    ///
    /// ```
    /// use oxidizer_rt::{RuntimeBuilder, BasicThreadState};
    /// use oxidizer_time::runtime::InactiveClock;
    /// use oxidizer_time::ClockControl;
    ///
    /// // Automatically advance all timers without waiting.
    /// let clock_control = ClockControl::default().auto_advance_timers(true);
    ///
    /// // Use the clock control in the runtime.
    /// let runtime = RuntimeBuilder::new::<BasicThreadState>().with_clock(clock_control).build();
    /// ```
    #[must_use]
    pub fn with_clock(mut self, clock: impl Into<InactiveClock>) -> Self {
        self.clock = clock.into();
        self
    }

    /// Builds and starts a new instance of the Oxidizer Runtime.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn build(self) -> Result<Runtime<TS>, TS::Error> {
        let processor_set = processor_set_from_config(&self.processor_config);
        let processors = processor_set.to_processors();

        let domains = create_domains(processor_set.count());

        let mut async_worker_command_txs = Vec::with_capacity(processor_set.count());
        let mut async_worker_start_txs = Vec::with_capacity(processor_set.count());
        let mut async_worker_success_rxs = Vec::with_capacity(processor_set.count());

        let mut async_worker_join_handles = Vec::with_capacity(processor_set.count());

        // This Arc is only used at startup (or when calling block_on), so it's ok from the perf point of view.
        let arg_types = Arc::new(self.shared_state);

        for (worker_index, processor) in processors.enumerate() {
            let (command_tx, command_rx) = mpsc::channel();
            let (waker_tx, waker_rx) = mpsc::channel();
            let (start_tx, start_rx) = oneshot::channel();
            let (success_tx, success_rx) = oneshot::channel();
            let domain = domains[worker_index];

            async_worker_start_txs.push(start_tx);

            async_worker_join_handles.push(
                AsyncWorkerStartInfo {
                    command_rx,
                    start_rx,
                    success_tx,
                    clock: self.clock.clone(),
                    shared_state: Arc::clone(&arg_types),
                    domain,
                    waker_tx,
                    processor,
                }
                .start(),
            );

            let waker = waker_rx.recv().expect("No waker received");
            async_worker_command_txs.push((command_tx, waker, domain));
            async_worker_success_rxs.push(success_rx);
        }

        let dispatcher = Arc::new(DispatcherCore::new(
            ThreadWaiter::new(async_worker_join_handles),
            NonEmpty::from_vec(async_worker_command_txs).expect(
                "the number is either hardcoded or validated in the builder, so can never be zero",
            ),
        ));

        let dispatcher_client = DispatcherClient::new(dispatcher);

        for start_tx in async_worker_start_txs {
            start_tx
                .send(StartWorker {
                    dispatcher: dispatcher_client.clone(),
                })
                .expect(
                    "failed to send start signal to worker thread - runtime in unrecoverable state",
                );
        }

        for success_rx in async_worker_success_rxs {
            let signal = success_rx
                .recv()
                .expect("failed to receive worker startup signal");

            if let Err(err) = signal {
                use crate::DispatchStop;
                dispatcher_client.stop();

                event!(Level::ERROR, "runtime failed to start");

                return Err(err);
            }
        }

        event!(Level::DEBUG, "runtime started");

        Ok(Runtime::with_dispatcher(dispatcher_client))
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeBuilder {
    /// Creates a new builder with the default configuration for the current operating environment. This
    /// builder will use the default config for the given thread state type.
    #[must_use]
    pub fn new<TS>() -> RuntimeBuilder<TS>
    where
        TS: RuntimeThreadState,
        TS::SharedState: Default,
    {
        RuntimeBuilder {
            shared_state: Default::default(),
            clock: InactiveClock::default(),
            processor_config: ResourceQuota::default(),
        }
    }

    /// Creates a new builder with the default configuration for the current operating environment. This
    /// builder will use the provided config for the thread state type.
    pub fn with_shared_state<TS>(shared_state: impl Into<TS::SharedState>) -> RuntimeBuilder<TS>
    where
        TS: RuntimeThreadState,
    {
        RuntimeBuilder {
            shared_state: shared_state.into(),
            clock: InactiveClock::default(),
            processor_config: ResourceQuota::default(),
        }
    }
}

/// The data set required to start one async worker (the message channels and associated data).
/// These are the inputs necessary to initialize one worker thread.
#[derive(Debug)]
struct AsyncWorkerStartInfo<TS>
where
    TS: RuntimeThreadState,
{
    command_rx: mpsc::Receiver<AsyncWorkerCommand<TS>>,
    start_rx: oneshot::Receiver<StartWorker<TS>>,
    success_tx: oneshot::Sender<Result<(), TS::Error>>,
    shared_state: Arc<TS::SharedState>,
    clock: InactiveClock,
    domain: Domain,
    waker_tx: Sender<WakerFacade>,
    processor: Processor,
}

impl<TS> AsyncWorkerStartInfo<TS>
where
    TS: RuntimeThreadState,
{
    fn start(self) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            non_blocking_thread::flag_current_thread();

            //Pin the thread to it's assigned processor.
            let processor_set = ProcessorSet::from_processor(self.processor);
            processor_set.pin_current_thread_to();

            // SAFETY: We are not allowed to drop this without going through the proper shutdown
            // process that first drains the queue and then explicitly shuts it down. We do this
            // upon `Worker::run()` processing the shutdown command.
            let spawn_queue = unsafe { SpawnQueue::new() };

            let system_worker = SystemWorker::new();
            let sys_worker_clone = Rc::clone(&system_worker);

            let (waker, waker_waiter, io_context) = build_io(&spawn_queue);

            self.waker_tx
                .send(waker.clone())
                .expect("Could not send waker to worker");

            // The start command is sent to all threads by the builder when all the threads have
            // started. This command exists because the builder first needs to collect all the
            // thread JoinHandles before it can create the dispatcher logic that all the workers
            // will share to send commands to each other.
            let start_signal = self
                .start_rx
                .recv()
                .expect("RuntimeBuilder failed between initializing and starting a worker - impossible to continue worker execution");

            let dispatcher = Rc::new(start_signal.dispatcher);

            // SAFETY: We are not allowed to drop the executor until it reports that it has
            // completed shutdown. This is guaranteed by AsyncWorker, as long
            // as we call `AsyncWorker::run()`, which we do.
            let executor = unsafe { AsyncTaskExecutor::new(waker) };

            let thread_state_constructor = async move |queue, clock| {
                let core_builtins = CoreRuntimeBuiltins::new(
                    clock,
                    &queue,
                    &sys_worker_clone,
                    Rc::<DispatcherClient<TS>>::clone(&dispatcher),
                    self.domain,
                    processor_set,
                    io_context,
                );

                let shared_init_state =
                    TS::async_init(&self.shared_state, core_builtins.clone()).await?;

                TS::sync_init(
                    &self.shared_state,
                    shared_init_state,
                    RuntimeBuiltins::new(&dispatcher, core_builtins.clone(), self.domain),
                )
            };

            // SAFETY: We are required to call `.run()` before dropping it, which we do.
            let worker: AsyncWorker<AsyncTaskExecutor, TS> = unsafe {
                AsyncWorker::new(
                    self.command_rx,
                    executor,
                    thread_state_constructor,
                    Rc::clone(&system_worker),
                    self.clock,
                    waker_waiter,
                    spawn_queue,
                    self.success_tx,
                )
            };

            worker.run();

            system_worker.join();
        })
    }
}

/// This is the signal to start a worker. We send this signal to each worker when all the
/// workers are initialized and the runtime is essentially ready to operate. Task processing starts
/// after this signal is received by a worker.
///
/// Work may already get enqueued before this signal is received by a specific worker!
#[derive(Debug)]
struct StartWorker<TS>
where
    TS: RuntimeThreadState,
{
    dispatcher: DispatcherClient<TS>,
}

#[cfg_attr(test, mutants::skip)]
fn build_io(
    spawn_queue: &Rc<SpawnQueue>,
) -> (WakerFacade, WakerWaiterFacade, oxidizer_io::Context) {
    use crate::IoDispatch;

    let io_dispatch = IoDispatch::new(Rc::downgrade(spawn_queue));

    // SAFETY: We are not allowed to drop this without cleaning it up, which we do.
    let driver: oxidizer_io::Driver = unsafe { oxidizer_io::Driver::new(Box::new(io_dispatch)) };

    let waker = driver.waker();
    let io = driver.context().clone();

    (waker.into(), driver.into(), io)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxidizer_time::ClockControl;

    use super::*;

    #[test]
    fn custom_clock_ok() {
        let clock_control = ClockControl::default();
        let builder = RuntimeBuilder::new::<BasicThreadState>().with_clock(clock_control.clone());

        let (clock, _) = builder.clock.activate();
        let now = clock.now();

        clock_control.advance(Duration::from_secs(1));

        assert_eq!(
            clock.now().checked_duration_since(now).unwrap(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn processor_config_should_set_correctly() {
        let config = ResourceQuota::new().with_num_processors(2);

        let builder = RuntimeBuilder::new::<BasicThreadState>().with_resource_quota(config.clone());
        assert_eq!(builder.processor_config, config);
    }
}