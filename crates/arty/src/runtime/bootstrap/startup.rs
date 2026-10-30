// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_io_host::IoRuntime;
use many_cpus::{ProcessorSet, SystemHardware};
use nonempty::NonEmpty;
use thread_aware::ThreadBuilder;
use tick::runtime::InactiveClock;

use crate::runtime::config::RuntimeConfig;
use crate::runtime::dispatch::{DispatcherClient, DispatcherCore};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;
use crate::runtime::shutdown::Shutdown;
use crate::runtime::worker::{WorkerJoiner, asynchronous};

pub(crate) fn build(processor_config: RuntimeConfig, clock: &InactiveClock, thread_builder: &ThreadBuilder) -> Result<Runtime, Error> {
    let available = available_processors();
    build_with_processors(processor_config, clock, thread_builder, &available)
}

fn build_with_processors(
    processor_config: RuntimeConfig,
    clock: &InactiveClock,
    thread_builder: &ThreadBuilder,
    available: &ProcessorSet,
) -> Result<Runtime, Error> {
    let processors = processor_config.workers_policy.select(available)?;

    let mut workers = Vec::with_capacity(processors.len());

    // Respect the RUST_MIN_STACK environment variable if set and larger than the configured stack size.
    let stack_size = processor_config
        .stack_size
        .get()
        .max(std::env::var("RUST_MIN_STACK").unwrap_or_default().parse().unwrap_or(0));
    let monitor_options = processor_config.monitor_options;
    let panic_policy = processor_config.panic_policy;

    let shutdown = Shutdown::default();
    let io_runtime = IoRuntime::new(observed::Sink::noop());
    let blocking_pools = processor_config.blocking_pool_policy.into_pools()?;

    for (worker_index, processor) in processors.into_iter().enumerate() {
        workers.push(asynchronous::start(asynchronous::StartOptions {
            stack_size,
            inactive_clock: clock.clone(),
            processor,
            worker_index,
            thread_builder: thread_builder.clone(),
            blocking_pools: blocking_pools.clone(),
            shutdown: shutdown.clone(),
            io_runtime: io_runtime.clone(),
            monitor_options: monitor_options.for_worker(),
            panic_policy: panic_policy.clone(),
        }));
    }

    let dispatcher = DispatcherCore::new(
        NonEmpty::from_vec(workers.iter().map(|worker| worker.endpoint().clone()).collect())
            .expect("the number is either hardcoded or validated in the builder, so can never be zero"),
        shutdown,
    );

    let dispatcher_client = DispatcherClient::new(dispatcher);

    let mut join_handles = Vec::with_capacity(workers.len());
    for worker in workers {
        join_handles.push(worker.activate(dispatcher_client.clone()));
    }
    let worker_joiner = WorkerJoiner::new(join_handles);

    Ok(Runtime::with_dispatcher(dispatcher_client, worker_joiner))
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
    use std::task::Waker;
    use std::thread;
    use std::thread::ThreadId;

    use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
    use many_cpus::fake::HardwareBuilder;
    use performables::arc::{Arc, PerThread};
    use thread_aware::{Thread, ThreadAware};

    use super::*;
    use crate::runtime::config::MonitorOptions;
    use crate::runtime::{BlockingPoolPolicy, MonitorPolicy, PanicPolicy, WorkersPolicy};

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
        let runtime = runtime_with_processors(
            WorkersPolicy::exactly(WORKERS),
            MonitorPolicy::enabled().into(),
            &hardware.processors(),
        );
        let cx = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let scheduler = cx.scheduler().clone();
        let counts = DriverCounts {
            initialized: StdArc::new(AtomicUsize::new(0)),
            stopped: StdArc::new(AtomicUsize::new(0)),
        };
        let per_thread = Arc::<AtomicUsize, PerThread>::new_with(AtomicUsize::default);
        let tasks: Vec<_> = cx
            .control()
            .workers()
            .into_iter()
            .map(|worker| {
                scheduler.spawn_at(
                    worker,
                    (cx.clone(), counts.clone(), per_thread.clone(), HardwareProbe(hardware.clone())),
                    |(cx, counts, value, hardware)| async move {
                        let thread = thread::current().id();
                        assert_eq!(cx.thread().id(), thread);
                        let _context = cx.io().context::<TestContext>(&counts);
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
    }

    #[test]
    fn maximum_workers_remain_bounded_by_fake_processor_capacity() {
        const PROCESSORS: usize = 4;

        let hardware = fake_hardware(PROCESSORS);
        let runtime = runtime_with_processors(
            WorkersPolicy::at_most(16),
            MonitorPolicy::disabled().into(),
            &hardware.processors(),
        );
        let cx = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let workers: HashSet<_> = cx
            .control()
            .workers()
            .into_iter()
            .map(|worker| cx.scheduler().spawn_at(worker, (), |()| async move { thread::current().id() }))
            .map(|task| task.join().unwrap())
            .collect();

        assert_eq!(workers.len(), PROCESSORS);
        runtime.stop().unwrap();
    }

    // Prove that all-worker startup creates distinct running workers while the
    // fake topology keeps processor capacity independent of the test host.
    #[test]
    fn all_workers_use_fake_processor_capacity() {
        const PROCESSORS: usize = 4;

        let hardware = fake_hardware(PROCESSORS);
        let runtime = runtime_with_processors(
            WorkersPolicy::all(),
            MonitorPolicy::disabled().into(),
            &hardware.processors(),
        );
        let cx = runtime.scheduler().block_on(async |cx| cx).unwrap();
        let workers: HashSet<_> = cx
            .control()
            .workers()
            .into_iter()
            .map(|worker| cx.scheduler().spawn_at(worker, (), |()| async move { thread::current().id() }))
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

    fn runtime_with_processors(policy: WorkersPolicy, monitor_options: MonitorOptions, processors: &ProcessorSet) -> Runtime {
        build_with_processors(
            RuntimeConfig {
                workers_policy: policy,
                stack_size: NonZero::new(2 * 1024 * 1024).unwrap(),
                blocking_pool_policy: BlockingPoolPolicy::shared(),
                monitor_options,
                panic_policy: PanicPolicy::default(),
            },
            &InactiveClock::new_isolated(),
            &ThreadBuilder::default(),
            processors,
        )
        .unwrap()
    }
}
