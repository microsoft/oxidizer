// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::AtomicBool;
use std::task::Waker;
use std::thread;

use arty_io_host::IoRuntime;
use many_cpus::{ProcessorSet, SystemHardware};
use nonempty::NonEmpty;
use observed::{Sink, emit};
use performables::arc::Arc;
use performables::sync::channel;
use performables::sync::channel::{OneshotReceiver, Sender};
use thread_aware::{Thread, ThreadAware, ThreadBuilder};
use tick::runtime::InactiveClock;

use crate::io::{Io, RuntimeDriverHost};
use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::bootstrap::pools::BlockingPools;
use crate::runtime::config::RuntimeConfig;
use crate::runtime::dispatch::{DispatcherClient, DispatcherCore, WorkerEndpoint};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;
use crate::runtime::telemetry::events::{
    AsyncWorkerActive, AsyncWorkerStarted, AsyncWorkerStopped, BlockingWorkerPoolMode, RuntimeStartFailed, RuntimeStarted,
};
use crate::runtime::thread::spawn;
use crate::runtime::thread::waiter::ThreadWaiter;
use crate::runtime::worker::AsyncWorker;
use crate::runtime::worker::protocol::AsyncWorkerCommand;
use crate::runtime::worker::signal::WorkerSignal;

pub(crate) fn build(
    processor_config: RuntimeConfig,
    clock: &InactiveClock,
    sink: Sink,
    thread_builder: &ThreadBuilder,
) -> Result<Runtime, Error> {
    let available = available_processors();
    build_with_processors(processor_config, clock, sink, thread_builder, &available)
}

fn build_with_processors(
    processor_config: RuntimeConfig,
    clock: &InactiveClock,
    sink: Sink,
    thread_builder: &ThreadBuilder,
    available: &ProcessorSet,
) -> Result<Runtime, Error> {
    let pool_mode = processor_config.blocking_pool_policy.mode_label();
    let processors = processor_config.workers_policy.select(available).inspect_err(|_| {
        emit!(
            &sink,
            RuntimeStartFailed {
                blocking_worker_pool_mode: BlockingWorkerPoolMode(pool_mode),
            }
        );
    })?;
    let processors_used = processors.processors_used();

    let mut async_worker_command_txs = Vec::with_capacity(processors.len());
    let mut async_worker_start_txs = Vec::with_capacity(processors.len());
    let mut async_worker_success_rxs = Vec::with_capacity(processors.len());

    let mut async_worker_join_handles = Vec::with_capacity(processors.len());

    // Respect the RUST_MIN_STACK environment variable if set and larger than the configured stack size.
    let stack_size = processor_config
        .stack_size
        .get()
        .max(std::env::var("RUST_MIN_STACK").unwrap_or_default().parse().unwrap_or(0));

    let shutdown_started = Arc::new(AtomicBool::new(false));
    let io_runtime = IoRuntime::new(sink.clone());
    let blocking_pools = processor_config.blocking_pool_policy.into_pools().inspect_err(|_| {
        emit!(
            &sink,
            RuntimeStartFailed {
                blocking_worker_pool_mode: BlockingWorkerPoolMode(pool_mode),
            }
        );
    })?;

    for (worker_index, processor) in processors.into_iter().enumerate() {
        let (command_tx, command_rx) = channel::unbounded();
        let (worker_endpoint_tx, worker_endpoint_rx) = channel::unbounded();
        let (start_tx, start_rx) = channel::oneshot();
        let (success_tx, success_rx) = channel::unbounded();

        async_worker_start_txs.push(start_tx);

        async_worker_join_handles.push(
            AsyncWorkerStartInfo {
                command_rx,
                stack_size,
                start_rx,
                success_tx,
                inactive_clock: clock.clone(),
                processor,
                worker_index,
                worker_endpoint_tx,
                thread_builder: thread_builder.clone(),
                blocking_pools: blocking_pools.clone(),
                shutdown_started: Arc::clone(&shutdown_started),
                io_runtime: io_runtime.clone(),
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
    let dispatcher = Arc::new(DispatcherCore::new_with_shutdown(
        ThreadWaiter::new(async_worker_join_handles),
        NonEmpty::from_vec(async_worker_command_txs)
            .expect("the number is either hardcoded or validated in the builder, so can never be zero"),
        sink,
        shutdown_started,
    ));

    let dispatcher_client = DispatcherClient::new(dispatcher);

    for start_tx in async_worker_start_txs {
        start_tx
            .send(StartWorker {
                dispatcher: dispatcher_client.clone(),
            })
            .expect("a starting worker retains its start receiver");
    }

    for success_rx in async_worker_success_rxs {
        success_rx.recv().expect("failed to receive worker startup signal");
    }

    emit!(
        runtime_sink,
        RuntimeStarted {
            processors_available: available.len().into(),
            processors_used: processors_used.into(),
            blocking_worker_pool_mode: BlockingWorkerPoolMode(pool_mode),
            stack_size_bytes: stack_size.into(),
        }
    );

    Ok(Runtime::with_dispatcher(dispatcher_client))
}

/// The data set required to start one async worker (the message channels and associated data).
/// These are the inputs necessary to initialize one worker thread.
#[derive(Debug)]
struct AsyncWorkerStartInfo {
    command_rx: channel::Receiver<AsyncWorkerCommand>,
    stack_size: usize,
    start_rx: OneshotReceiver<StartWorker>,
    success_tx: Sender<()>,
    inactive_clock: InactiveClock,
    processor: ProcessorSet,
    worker_index: usize,
    worker_endpoint_tx: Sender<(Waker, Thread, Arc<BlockingWorker>)>,
    thread_builder: ThreadBuilder,
    blocking_pools: BlockingPools,
    shutdown_started: Arc<AtomicBool>,
    io_runtime: IoRuntime,
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
            inactive_clock,
            processor,
            worker_index,
            worker_endpoint_tx,
            thread_builder,
            blocking_pools,
            shutdown_started,
            io_runtime,
            sink,
        } = self;

        let worker_sink = sink.clone();
        // Pin the thread to its assigned processor.
        let thread_builder = thread_builder.with_numa_node(processor.processors().first().memory_region_id());
        processor.pin_current_thread_to();
        let current = thread_builder.build(thread::current().id());

        // The constructing thread is outside the runtime, so the relocation source is unknown.
        let mut clock = inactive_clock;
        clock.relocate(None, &current);

        // Use shared blocking worker pool if shared, otherwise use a new one
        let blocking_worker =
            BlockingWorker::new_with_shutdown(blocking_pools.build_worker(), worker_sink.clone(), Arc::clone(&shutdown_started));

        let signal = Arc::new(WorkerSignal::default());
        let worker_waker = WorkerSignal::waker(&signal);

        worker_endpoint_tx
            .send((worker_waker, current.clone(), Arc::clone(&blocking_worker)))
            .expect("failed to send the worker endpoint to the runtime builder");

        // The start command is sent to all threads by bootstrap when all the threads have
        // started. This command exists because bootstrap first needs to collect all the
        // thread JoinHandles before it can create the dispatcher logic that all the workers
        // will share to send commands to each other.
        let start_signal = futures::executor::block_on(start_rx)
            .expect("RuntimeBuilder failed between initializing and starting a worker - impossible to continue worker execution");
        let dispatcher = start_signal.dispatcher;
        let io = Io::new(io_runtime.clone(), dispatcher.clone(), current.clone());
        let driver_waker = RuntimeDriverHost::install(&io_runtime, dispatcher.clone(), &current, WorkerSignal::waker(&signal));
        signal.set_driver_waker(driver_waker);

        let worker = AsyncWorker::new(
            command_rx,
            dispatcher,
            current,
            sink,
            io,
            Arc::clone(&blocking_worker),
            clock,
            signal,
        );

        emit!(
            worker_sink,
            AsyncWorkerStarted {
                processor_index: worker_index.into(),
            }
        );
        emit!(worker_sink, AsyncWorkerActive { delta: 1 });
        _ = success_tx.send(());

        // Incomplete executor teardown fails closed. A panic after safe retirement still
        // needs to join the blocking pool before the worker's failure is reported.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run()));
        if outcome.is_ok() {
            emit!(
                worker_sink,
                AsyncWorkerStopped {
                    processor_index: worker_index.into(),
                }
            );
        }
        emit!(worker_sink, AsyncWorkerActive { delta: -1 });

        blocking_worker.join();
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashSet;
    use std::num::NonZero;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread::ThreadId;

    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
    use many_cpus::fake::HardwareBuilder;
    use observed_testing::{TEST_ID, test_emitter};
    use performables::arc::PerThread;

    use super::*;
    use crate::runtime::{BlockingPoolPolicy, WorkersPolicy};

    thread_local! {
        static WORKER_LOCAL_CALLS: AtomicUsize = const { AtomicUsize::new(0) };
    }

    #[derive(Clone)]
    struct DriverCounts {
        initialized: StdArc<AtomicUsize>,
        stopped: StdArc<AtomicUsize>,
    }

    impl ThreadAware for DriverCounts {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[derive(Clone)]
    struct HardwareProbe(SystemHardware);

    impl ThreadAware for HardwareProbe {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    #[derive(Clone)]
    struct TestContext {
        counts: DriverCounts,
    }

    impl ThreadAware for TestContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for TestContext {
        type ContextOptions = DriverCounts;
        type Driver = TestDriver;

        fn context(options: Self::ContextOptions) -> Self {
            Self { counts: options }
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            self.counts.initialized.fetch_add(1, Ordering::SeqCst);
            Ok(TestDriver {
                stopped: StdArc::clone(&self.counts.stopped),
            })
        }
    }

    struct TestDriver {
        stopped: StdArc<AtomicUsize>,
    }

    impl Driver for TestDriver {
        fn waker(&self) -> Waker {
            Waker::noop().clone()
        }

        fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn exact_workers_oversubscribe_fake_processors_with_isolated_worker_state() {
        const PROCESSORS: usize = 4;
        const WORKERS: usize = 16;

        let hardware = fake_hardware(PROCESSORS);
        let (sink, processor) = test_emitter(TEST_ID);
        let runtime = runtime_with_processors(WorkersPolicy::exactly(WORKERS), &hardware.processors(), sink);
        let builtins = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let scheduler = builtins.scheduler().clone();
        let counts = DriverCounts {
            initialized: StdArc::new(AtomicUsize::new(0)),
            stopped: StdArc::new(AtomicUsize::new(0)),
        };
        let per_thread = Arc::<AtomicUsize, PerThread>::new_with(AtomicUsize::default);
        let tasks: Vec<_> = builtins
            .control()
            .workers()
            .into_iter()
            .map(|worker| {
                scheduler.spawn_at(
                    worker,
                    (
                        builtins.clone(),
                        counts.clone(),
                        per_thread.clone(),
                        HardwareProbe(hardware.clone()),
                    ),
                    |(builtins, counts, value, hardware)| async move {
                        let thread = thread::current().id();
                        assert_eq!(builtins.thread().id(), thread);
                        let _context = builtins.io().context::<TestContext>(&counts);
                        let local_calls = WORKER_LOCAL_CALLS.with(|calls| calls.fetch_add(1, Ordering::SeqCst));
                        let per_thread_calls = value.fetch_add(1, Ordering::SeqCst);
                        (thread, hardware.0.current_processor_id(), local_calls, per_thread_calls)
                    },
                )
            })
            .collect();
        assert_eq!(tasks.len(), WORKERS);
        let outcomes: Vec<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();

        assert_eq!(
            outcomes
                .iter()
                .map(|(thread, _, _, _)| *thread)
                .collect::<HashSet<ThreadId>>()
                .len(),
            WORKERS
        );
        assert_eq!(
            outcomes.iter().map(|(_, processor, _, _)| *processor).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3]
        );
        assert!(
            outcomes
                .iter()
                .all(|(_, _, local_calls, per_thread_calls)| (*local_calls, *per_thread_calls) == (0, 0))
        );
        assert_eq!(counts.initialized.load(Ordering::SeqCst), WORKERS);

        runtime.stop().unwrap();
        assert_eq!(counts.stopped.load(Ordering::SeqCst), WORKERS);
        let started = processor
            .events()
            .into_iter()
            .find(|event| event.name() == "arty.rt.started")
            .unwrap();
        let dimensions = started.dimensions();
        assert!(dimensions.contains(&("processors.available".into(), PROCESSORS.to_string().into())));
        assert!(dimensions.contains(&("processors.used".into(), PROCESSORS.to_string().into())));
    }

    #[test]
    fn maximum_workers_remain_bounded_by_fake_processor_capacity() {
        const PROCESSORS: usize = 4;

        let hardware = fake_hardware(PROCESSORS);
        let runtime = runtime_with_processors(WorkersPolicy::at_most(16), &hardware.processors(), Sink::noop());
        let builtins = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let workers: HashSet<_> = builtins
            .control()
            .workers()
            .into_iter()
            .map(|worker| {
                builtins
                    .scheduler()
                    .spawn_at(worker, (), |()| async move { thread::current().id() })
            })
            .map(|task| task.join().unwrap())
            .collect();

        assert_eq!(workers.len(), PROCESSORS);
        runtime.stop().unwrap();
    }

    #[test]
    fn all_workers_use_fake_processor_capacity() {
        const PROCESSORS: usize = 4;

        let hardware = fake_hardware(PROCESSORS);
        let runtime = runtime_with_processors(WorkersPolicy::all(), &hardware.processors(), Sink::noop());
        let builtins = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let workers: HashSet<_> = builtins
            .control()
            .workers()
            .into_iter()
            .map(|worker| {
                builtins
                    .scheduler()
                    .spawn_at(worker, (), |()| async move { thread::current().id() })
            })
            .map(|task| task.join().unwrap())
            .collect();

        assert_eq!(workers.len(), PROCESSORS);
        runtime.stop().unwrap();
    }

    fn fake_hardware(processors: usize) -> SystemHardware {
        SystemHardware::fake(HardwareBuilder::from_counts(
            NonZero::new(processors).unwrap(),
            NonZero::new(1).unwrap(),
        ))
    }

    fn runtime_with_processors(policy: WorkersPolicy, processors: &ProcessorSet, sink: Sink) -> Runtime {
        build_with_processors(
            RuntimeConfig {
                workers_policy: policy,
                stack_size: NonZero::new(2 * 1024 * 1024).unwrap(),
                blocking_pool_policy: BlockingPoolPolicy::shared(),
            },
            &InactiveClock::default(),
            sink,
            &ThreadBuilder::default(),
            processors,
        )
        .unwrap()
    }
}
