// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock, mpsc};
use std::task::Waker;
use std::thread;

use events_once::{BoxedReceiver, Event};
use many_cpus::{ProcessorSet, SystemHardware};
use nonempty::NonEmpty;
use observed::{Sink, emit};
use thread_aware::{Thread, ThreadAware, ThreadBuilder};
use tick::runtime::InactiveClock;

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::bootstrap::pools::BlockingPools;
use crate::runtime::config::RuntimeConfig;
use crate::runtime::context::{CoreRuntimeBuiltins, RuntimeBuiltins, SharedState};
use crate::runtime::dispatch::{DispatcherClient, DispatcherCore, WorkerEndpoint};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;
use crate::runtime::telemetry::events::{
    AsyncWorkerActive, AsyncWorkerStarted, AsyncWorkerStopped, BlockingWorkerPoolMode, RuntimeStartFailed, RuntimeStarted,
};
use crate::runtime::thread::waiter::ThreadWaiter;
use crate::runtime::thread::{flag_current_thread, spawn};
use crate::runtime::worker::AsyncWorker;
use crate::runtime::worker::protocol::AsyncWorkerCommand;
use crate::runtime::worker::signal::WorkerSignal;
use crate::task::Builtins;

pub(in crate::runtime) fn build(
    processor_config: RuntimeConfig,
    clock: &InactiveClock,
    sink: Sink,
    thread_builder: &ThreadBuilder,
) -> Result<Runtime, Error> {
    let pool_mode = processor_config.blocking_pool_policy.mode_label();
    let available = available_processors();
    let processors = processor_config.num_processors.select(&available).inspect_err(|_| {
        emit!(
            &sink,
            RuntimeStartFailed {
                blocking_worker_pool_mode: BlockingWorkerPoolMode(pool_mode),
            }
        );
    })?;
    // Decomposition yields singleton processor sets, one per worker. Sort by their sole
    // processor's ID so startup endpoints and service slots share a stable worker ordering.
    let mut processors: Vec<_> = processors.decompose().into_iter().collect();
    processors.sort_by_key(|processor| processor.processors().first().id());

    let mut async_worker_command_txs = Vec::with_capacity(processors.len());
    let mut async_worker_start_txs = Vec::with_capacity(processors.len());
    let mut async_worker_success_rxs = Vec::with_capacity(processors.len());

    let mut async_worker_join_handles = Vec::with_capacity(processors.len());

    // Respect the RUST_MIN_STACK environment variable if set and larger than the configured stack size.
    let stack_size = processor_config
        .stack_size
        .max(std::env::var("RUST_MIN_STACK").unwrap_or_default().parse().unwrap_or(0));

    let worker_count = processors.len();
    let shared_state: SharedState = (0..worker_count).map(|_| OnceLock::new()).collect();
    let blocking_pools = processor_config.blocking_pool_policy.into_pools();

    for (worker_index, processor) in processors.into_iter().enumerate() {
        let (command_tx, command_rx) = mpsc::channel();
        let (worker_endpoint_tx, worker_endpoint_rx) = mpsc::channel();
        let (start_tx, start_rx) = Event::boxed();
        let (success_tx, success_rx) = mpsc::channel();

        async_worker_start_txs.push(start_tx);

        async_worker_join_handles.push(
            AsyncWorkerStartInfo {
                command_rx,
                stack_size,
                start_rx,
                success_tx,
                inactive_clock: clock.clone(),
                shared_state: Arc::clone(&shared_state),
                processor,
                worker_index,
                worker_endpoint_tx,
                thread_builder: thread_builder.clone(),
                blocking_pools: blocking_pools.clone(),
                sink: sink.clone(),
            }
            .start(),
        );

        let (waker, thread, blocking_worker) = worker_endpoint_rx
            .recv()
            .expect("failed to receive the worker endpoint from the starting worker");
        async_worker_command_txs.push(WorkerEndpoint {
            command_tx,
            waker,
            thread,
            blocking_worker,
        });
        async_worker_success_rxs.push(success_rx);
    }

    let runtime_sink = sink.clone();
    let dispatcher = Arc::new(DispatcherCore::new(
        ThreadWaiter::new(async_worker_join_handles),
        NonEmpty::from_vec(async_worker_command_txs)
            .expect("the number is either hardcoded or validated in the builder, so can never be zero"),
        sink,
    ));

    let dispatcher_client = DispatcherClient::new(dispatcher);

    for start_tx in async_worker_start_txs {
        start_tx.send(StartWorker {
            dispatcher: dispatcher_client.clone(),
        });
    }

    for success_rx in async_worker_success_rxs {
        success_rx.recv().expect("failed to receive worker startup signal");
    }

    emit!(
        runtime_sink,
        RuntimeStarted {
            processors_available: available.len().into(),
            processors_used: worker_count.into(),
            blocking_worker_pool_mode: BlockingWorkerPoolMode(pool_mode),
            stack_size_bytes: stack_size.into(),
        }
    );

    Ok(Runtime::with_dispatcher(dispatcher_client, shared_state))
}

/// The data set required to start one async worker (the message channels and associated data).
/// These are the inputs necessary to initialize one worker thread.
#[derive(Debug)]
struct AsyncWorkerStartInfo {
    command_rx: mpsc::Receiver<AsyncWorkerCommand>,
    stack_size: usize,
    start_rx: BoxedReceiver<StartWorker>,
    success_tx: Sender<()>,
    shared_state: SharedState,
    inactive_clock: InactiveClock,
    processor: ProcessorSet,
    worker_index: usize,
    worker_endpoint_tx: Sender<(Waker, Thread, Arc<BlockingWorker>)>,
    thread_builder: ThreadBuilder,
    blocking_pools: BlockingPools,
    sink: Sink,
}

impl AsyncWorkerStartInfo {
    fn start(self) -> thread::JoinHandle<()> {
        let thread_name = format!("oxidizer-rt-{}", self.worker_index);
        let thread_lifecycle_sink = self.sink.clone();
        let stack_size = self.stack_size;

        spawn(&thread_lifecycle_sink, &thread_name, Some(stack_size), move || {
            self.run_worker_thread();
        })
    }

    fn run_worker_thread(self) {
        let Self {
            command_rx,
            stack_size: _stack_size,
            start_rx,
            success_tx,
            shared_state,
            inactive_clock,
            processor,
            worker_index,
            worker_endpoint_tx,
            thread_builder,
            blocking_pools,
            sink,
        } = self;

        let worker_sink = sink.clone();
        flag_current_thread();

        // Pin the thread to its assigned processor.
        let thread_builder = thread_builder.with_numa_node(processor.processors().first().memory_region_id());
        processor.pin_current_thread_to();
        let current = thread_builder.build(thread::current().id());

        // The constructing thread is outside the runtime, so the relocation source is unknown.
        let mut clock = inactive_clock;
        clock.relocate(None, &current);

        // Use shared blocking worker pool if shared, otherwise use a new one
        let blocking_worker = BlockingWorker::new(blocking_pools.build_worker(), worker_sink.clone());

        let signal = Arc::new(WorkerSignal::default());

        worker_endpoint_tx
            .send((Waker::from(Arc::clone(&signal)), current.clone(), Arc::clone(&blocking_worker)))
            .expect("failed to send the worker endpoint to the runtime builder");

        // The start command is sent to all threads by bootstrap when all the threads have
        // started. This command exists because bootstrap first needs to collect all the
        // thread JoinHandles before it can create the dispatcher logic that all the workers
        // will share to send commands to each other.
        let start_signal = futures::executor::block_on(async {
            start_rx
                .await
                .expect("RuntimeBuilder failed between initializing and starting a worker - impossible to continue worker execution")
        });

        let dispatcher = Rc::new(start_signal.dispatcher);

        let thread_state_constructor = {
            async move |tasks, clock| {
                let core_builtins = CoreRuntimeBuiltins::new(tasks, dispatcher.as_ref(), current.clone(), processor, sink);

                Builtins::sync_init(&shared_state, RuntimeBuiltins::new(&dispatcher, core_builtins, clock, current))
            }
        };

        // SAFETY: We are required to call `.run()` before dropping it, which we do.
        let worker: AsyncWorker = unsafe {
            AsyncWorker::new(
                command_rx,
                thread_state_constructor,
                Arc::clone(&blocking_worker),
                clock,
                signal,
                success_tx,
            )
        };

        emit!(
            worker_sink,
            AsyncWorkerStarted {
                processor_index: worker_index.into(),
            }
        );
        emit!(worker_sink, AsyncWorkerActive { delta: 1 });

        worker.run();

        emit!(
            worker_sink,
            AsyncWorkerStopped {
                processor_index: worker_index.into(),
            }
        );
        emit!(worker_sink, AsyncWorkerActive { delta: -1 });

        blocking_worker.join();
    }
}

/// This is the signal to start a worker. We send this signal to each worker when all the
/// workers are initialized and the runtime is essentially ready to operate. Task processing starts
/// after this signal is received by a worker.
///
/// Work may already get enqueued before this signal is received by a specific worker!
#[derive(Debug)]
struct StartWorker {
    dispatcher: DispatcherClient,
}

fn available_processors() -> ProcessorSet {
    #[cfg(all(miri, any(test, feature = "test-util")))]
    {
        use std::num::NonZeroUsize;
        SystemHardware::fake(many_cpus::fake::HardwareBuilder::from_counts(
            NonZeroUsize::new(6).expect("six is nonzero"),
            NonZeroUsize::new(2).expect("two is nonzero"),
        ))
        .processors()
    }
    #[cfg(not(all(miri, any(test, feature = "test-util"))))]
    {
        SystemHardware::current().processors()
    }
}
