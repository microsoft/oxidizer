// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::thread::{self, JoinHandle};

use arty_executor::discard_task_panic_payload;
use arty_io_host::IoRuntime;
use many_cpus::ProcessorSet;
use performables::arc::Arc;
use performables::sync::channel;
use performables::sync::channel::{OneshotReceiver, OneshotSender, Sender};
use thread_aware::{Thread as ThreadCoordinate, ThreadAware, ThreadBuilder};
use tick::runtime::InactiveClock;

use crate::io::{Io, RuntimeDriverHost};
use crate::runtime::config::{MonitorOptions, RuntimeName};
use crate::runtime::dispatch::{DispatcherClient, WorkerEndpoint};
use crate::runtime::error::Error;
use crate::runtime::hook::HookRegistration;
use crate::runtime::shutdown::Shutdown;
use crate::runtime::worker::{asynchronous, blocking};

/// Inputs required to spawn one asynchronous worker.
#[derive(Debug)]
pub(crate) struct StartOptions {
    pub(crate) stack_size: usize,
    pub(crate) inactive_clock: InactiveClock,
    pub(crate) processor: ProcessorSet,
    pub(crate) runtime_name: RuntimeName,
    pub(crate) worker_index: usize,
    pub(crate) thread_builder: ThreadBuilder,
    pub(crate) blocking_pools: blocking::Pools,
    pub(crate) shutdown: Shutdown,
    pub(crate) io_runtime: IoRuntime,
    pub(crate) monitor_options: MonitorOptions,
    pub(crate) hook_registrations: Vec<HookRegistration>,
}

/// A spawned worker waiting for runtime activation.
#[derive(Debug)]
pub(crate) struct SpawnedWorker {
    endpoint: WorkerEndpoint,
    activation_tx: OneshotSender<Activation>,
    ready_rx: channel::Receiver<()>,
    join_handle: JoinHandle<()>,
}

impl SpawnedWorker {
    pub(crate) fn endpoint(&self) -> &WorkerEndpoint {
        &self.endpoint
    }

    pub(crate) fn activate(self, dispatcher: DispatcherClient) -> JoinHandle<()> {
        self.activation_tx
            .send(Activation { dispatcher })
            .expect("a starting worker retains its activation receiver");
        self.ready_rx.recv().expect("failed to receive worker startup signal");
        self.join_handle
    }
}

pub(crate) fn start(options: StartOptions) -> SpawnedWorker {
    let (commands, command_rx) = asynchronous::command_channel();
    let (endpoint_tx, endpoint_rx) = channel::unbounded();
    let (activation_tx, activation_rx) = channel::oneshot();
    let (ready_tx, ready_rx) = channel::unbounded();
    let thread_settings = (options.runtime_name.async_worker(options.worker_index), options.stack_size);
    let thread_entry = ThreadEntry {
        options,
        command_rx,
        activation_rx,
        ready_tx,
        endpoint_tx,
    };
    let join_handle = spawn_worker_entry(&thread_settings, move || thread_entry.run(), spawn_thread);
    let endpoint = endpoint_rx
        .recv()
        .expect("failed to receive the worker endpoint from the starting worker");

    SpawnedWorker {
        endpoint: WorkerEndpoint {
            commands,
            waker: endpoint.waker,
            thread: endpoint.thread,
            blocking_worker: endpoint.blocking_worker,
        },
        activation_tx,
        ready_rx,
        join_handle,
    }
}

fn spawn_worker_entry<T, R>(settings: &(String, usize), task: T, spawn: impl FnOnce(&str, usize, T) -> R) -> R {
    spawn(&settings.0, settings.1, task)
}

#[derive(Debug)]
struct Activation {
    dispatcher: DispatcherClient,
}

#[derive(Debug)]
struct Endpoint {
    waker: Waker,
    thread: ThreadCoordinate,
    blocking_worker: Arc<blocking::Worker>,
}

#[derive(Debug)]
struct ThreadEntry {
    options: StartOptions,
    command_rx: asynchronous::CommandReceiver,
    activation_rx: OneshotReceiver<Activation>,
    ready_tx: Sender<()>,
    endpoint_tx: Sender<Endpoint>,
}

impl ThreadEntry {
    fn run(self) {
        let Self {
            options,
            command_rx,
            activation_rx,
            ready_tx,
            endpoint_tx,
        } = self;
        let StartOptions {
            inactive_clock,
            processor,
            worker_index,
            thread_builder,
            blocking_pools,
            shutdown,
            io_runtime,
            monitor_options,
            hook_registrations,
            ..
        } = options;

        // #[gamma::skip(iter.first_to_last, reason = "WorkerPlacements decomposes every selected processor set into singleton worker placements.")]
        let thread_builder = thread_builder.with_numa_node(processor.processors().first().memory_region_id());
        processor.pin_current_thread_to();
        let current = thread_builder.build(thread::current().id());
        asynchronous::LocalWorker::attach();

        let clock = prepare_worker_clock(inactive_clock, &current);

        let signal = Arc::new(asynchronous::Signal::default());
        let worker_waker = asynchronous::Signal::waker(&signal);
        let blocking_worker = blocking::Worker::new(blocking_pools.pool_for_worker(worker_index), shutdown);

        endpoint_tx
            .send(Endpoint {
                waker: worker_waker,
                thread: current.clone(),
                blocking_worker: Arc::clone(&blocking_worker),
            })
            .expect("failed to send the worker endpoint to the runtime builder");

        let dispatcher = futures::executor::block_on(activation_rx)
            .expect("RuntimeBuilder failed between initializing and activating a worker")
            .dispatcher;
        let io = Io::new(io_runtime.clone(), dispatcher.clone(), current.clone());
        let driver_waker = RuntimeDriverHost::install(&io_runtime, dispatcher.clone(), &current, asynchronous::Signal::waker(&signal));
        // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        signal.set_driver_waker(driver_waker);

        let worker = asynchronous::Worker::new(
            command_rx,
            dispatcher.clone(),
            current,
            io,
            clock,
            asynchronous::WorkerSignal(signal),
            monitor_options,
            hook_registrations,
        );

        _ = ready_tx.send(());

        let outcome = run_and_drop_worker(worker, &dispatcher, asynchronous::Worker::run);

        blocking_worker.join();
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}

fn prepare_worker_clock(mut clock: InactiveClock, current: &ThreadCoordinate) -> InactiveClock {
    clock.relocate(None, current);
    clock
}

fn run_and_drop_worker<W>(mut worker: W, dispatcher: &DispatcherClient, run: impl FnOnce(&mut W)) -> std::thread::Result<()> {
    let run_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut worker)));
    report_failure(&run_outcome, dispatcher);
    let drop_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(worker)));
    report_failure(&drop_outcome, dispatcher);
    combine_outcomes(run_outcome, drop_outcome)
}

fn spawn_thread(name: &str, stack_size: usize, task: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    spawn_thread_with(name, stack_size, task, native_spawn)
}

fn native_spawn(builder: thread::Builder, task: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    #[cfg(test)]
    if NATIVE_SPAWN_PROBE.get() {
        std::panic::resume_unwind(Box::new(NativeSpawnObserved(format!("{builder:?}"))));
    }
    builder
        .spawn(task)
        // Fallible construction and partial-startup cleanup are intentionally
        // deferred: https://github.com/microsoft/oxidizer/pull/785#discussion_r4183811801
        .expect("failed to spawn thread")
}

#[cfg(test)]
thread_local! {
    static NATIVE_SPAWN_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
struct NativeSpawnObserved(String);

fn spawn_thread_with<T, R>(name: &str, stack_size: usize, task: T, spawn: impl FnOnce(thread::Builder, T) -> R) -> R
where
    T: FnOnce() + Send + 'static,
{
    spawn(thread::Builder::new().name(name.to_string()).stack_size(stack_size), task)
}

fn report_failure(outcome: &std::thread::Result<()>, dispatcher: &DispatcherClient) {
    if outcome.is_err() {
        _ = dispatcher.stop(Error::worker_panicked().into_shutdown_cause());
    }
}

fn combine_outcomes(run: std::thread::Result<()>, teardown: std::thread::Result<()>) -> std::thread::Result<()> {
    match (run, teardown) {
        (Err(primary), Err(secondary)) => {
            discard_task_panic_payload(secondary);
            Err(primary)
        }
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(panic)) => Err(panic),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
// Public runtime construction cannot inject independent run and teardown outcomes.
mod tests {
    #[test]
    fn nov4_actual_spawn_wrapper_forwards_the_exact_stack_to_the_native_adapter() {
        for stack_size in [16_385, 32_769] {
            NATIVE_SPAWN_PROBE.set(true);
            let outcome = std::panic::catch_unwind(|| spawn_thread("nov4-native-request", stack_size, || {}));
            NATIVE_SPAWN_PROBE.set(false);
            let observed = outcome.unwrap_err().downcast::<NativeSpawnObserved>().unwrap();
            assert!(observed.0.contains("nov4-native-request"), "{}", observed.0);
            assert!(observed.0.contains(&format!("stack_size: Some({stack_size})")), "{}", observed.0);
        }
    }
    #[test]
    fn gamma_round2_worker_clock_preparation_isolates_each_workers_clock_owner_count() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Wake;

        #[derive(Default)]
        struct DriverWake(AtomicUsize);

        impl Wake for DriverWake {
            fn wake(self: StdArc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let builder = ThreadBuilder::default();
        let source = builder.build(thread::current().id());
        let destination = thread::spawn(move || builder.build(thread::current().id())).join().unwrap();
        let inactive = InactiveClock::new_isolated();
        let first = prepare_worker_clock(inactive.clone(), &source);
        let second = prepare_worker_clock(inactive, &destination);
        let wake = StdArc::new(DriverWake::default());
        let (first_clock, first_driver) = first.activate(Waker::from(StdArc::clone(&wake)));
        let (second_clock, second_driver) = second.activate(Waker::noop().clone());
        drop(first_clock);
        assert_eq!(wake.0.load(Ordering::Relaxed), 1);
        drop(second_clock);
        drop((first_driver, second_driver));
    }

    fn idle_endpoint() -> WorkerEndpoint {
        WorkerEndpoint {
            commands: asynchronous::command_channel().0,
            waker: Waker::noop().clone(),
            thread: ThreadBuilder::default().build(thread::current().id()),
            blocking_worker: blocking::Worker::new(blocking::Pool::new(RuntimeName::default(), None, None), Shutdown::default()),
        }
    }

    #[test]
    fn gamma_round2_worker_start_forwards_the_original_configured_stack_without_adjustment() {
        for stack_size in [16_384, 32_769] {
            let mut executions = 0;
            spawn_worker_entry(
                &(RuntimeName::new("requests").async_worker(3), stack_size),
                || executions += 1,
                |name, stack, mut task| {
                    assert_eq!(name, "requests-async-3");
                    assert_eq!(stack, stack_size);
                    task();
                },
            );
            assert_eq!(executions, 1);
        }
    }

    #[test]
    fn gamma_round2_activation_consumes_the_actual_buffered_ready_acknowledgment() {
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(idle_endpoint()), Shutdown::default()));
        let (activation_tx, activation_rx) = channel::oneshot();
        let (ready_tx, ready_rx) = channel::unbounded();
        let observer = ready_rx.clone();
        ready_tx.send(()).unwrap();
        let spawned = SpawnedWorker {
            endpoint: idle_endpoint(),
            activation_tx,
            ready_rx,
            join_handle: thread::spawn(|| {}),
        };
        let handle = spawned.activate(dispatcher);
        handle.join().unwrap();
        let mut activation = Box::pin(activation_rx);
        assert!(std::future::Future::poll(activation.as_mut(), &mut std::task::Context::from_waker(Waker::noop())).is_ready());
        assert!(observer.try_recv().is_err());
    }

    #[test]
    fn gamma_round2_thread_entry_reports_a_drop_failure_even_after_a_successful_run() {
        struct FailedDrop;

        impl Drop for FailedDrop {
            fn drop(&mut self) {
                panic!("round2 worker teardown failure");
            }
        }

        let shutdown = Shutdown::default();
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(idle_endpoint()), shutdown.clone()));
        let mut runs = 0;
        let result = run_and_drop_worker(FailedDrop, &dispatcher, |_| runs += 1);
        assert_eq!(runs, 1);
        assert_eq!(result.unwrap_err().downcast_ref::<&str>(), Some(&"round2 worker teardown failure"));
        assert_eq!(shutdown.error().unwrap().to_string(), "a runtime worker panicked");
    }

    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    use nonempty::NonEmpty;

    use super::*;

    #[test]
    fn gamma_round2_worker_thread_construction_preserves_the_exact_name_stack_and_task() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        for stack_size in [16_384, 32_769] {
            let completed = StdArc::new(AtomicUsize::new(0));
            let invoked = StdArc::clone(&completed);
            let builder = spawn_thread_with(
                "round2-worker",
                stack_size,
                move || {
                    invoked.fetch_add(1, Ordering::SeqCst);
                },
                |builder, task| {
                    task();
                    format!("{builder:?}")
                },
            );
            assert!(builder.contains("round2-worker"), "{builder}");
            assert!(builder.contains(&format!("stack_size: Some({stack_size})")), "{builder}");
            assert_eq!(completed.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn gamma_survivor_private_worker_outcomes_keep_primary_and_dispose_panicking_secondary_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Payload(StdArc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                panic!("secondary worker payload");
            }
        }
        let drops = StdArc::new(AtomicUsize::new(0));
        let outcome =
            std::panic::catch_unwind(|| combine_outcomes(Err(Box::new("primary")), Err(Box::new(Payload(StdArc::clone(&drops))))));
        let primary = outcome.unwrap().unwrap_err().downcast::<&str>().unwrap();
        assert_eq!(*primary, "primary");
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    use crate::runtime::dispatch::DispatcherCore;

    #[test]
    fn spawn_executes_task_successfully() {
        let result = StdArc::new(StdMutex::new(0));
        let handle = spawn_thread("test-thread", 2 * 1024 * 1024, {
            let result = StdArc::clone(&result);
            move || *result.lock().unwrap() = 42
        });

        handle.join().unwrap();

        assert_eq!(*result.lock().unwrap(), 42);
    }

    #[test]
    fn spawn_propagates_panic() {
        let handle = spawn_thread("panicking-thread", 2 * 1024 * 1024, || {
            panic!("task panicked");
        });

        handle.join().unwrap_err();
    }

    #[test]
    fn spawn_names_the_thread() {
        const THREAD_NAME: &str = "named-thread";
        let handle = spawn_thread(THREAD_NAME, 2 * 1024 * 1024, || {
            assert_eq!(thread::current().name(), Some(THREAD_NAME));
        });

        handle.join().unwrap();
    }

    #[test]
    fn run_panic_precedes_teardown_panic() {
        let outcome = combine_outcomes(Err(Box::new("run panic")), Err(Box::new("teardown panic")));
        assert_eq!(*outcome.unwrap_err().downcast::<&str>().unwrap(), "run panic");
    }

    #[test]
    fn teardown_panic_is_reported_after_successful_run() {
        let outcome = combine_outcomes(Ok(()), Err(Box::new("teardown panic")));
        assert_eq!(*outcome.unwrap_err().downcast::<&str>().unwrap(), "teardown panic");
    }

    #[test]
    fn successful_run_and_teardown_succeed() {
        combine_outcomes(Ok(()), Ok(())).unwrap();
    }

    #[test]
    fn run_panic_is_reported_without_teardown_panic() {
        let outcome = combine_outcomes(Err(Box::new("run panic")), Ok(()));
        assert_eq!(*outcome.unwrap_err().downcast::<&str>().unwrap(), "run panic");
    }

    #[test]
    fn failure_publishes_shutdown_cause() {
        let (commands, _command_rx) = asynchronous::command_channel();
        let shutdown = Shutdown::default();
        let current = ThreadBuilder::default().build(thread::current().id());
        let blocking_worker = blocking::Worker::new(blocking::Pool::new(RuntimeName::default(), None, None), shutdown.clone());
        let endpoint = crate::runtime::testing::worker_endpoint(commands, Waker::noop().clone(), current, blocking_worker);
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(endpoint), shutdown.clone()));

        report_failure(&Err(Box::new("worker panic")), &dispatcher);

        assert_eq!(shutdown.error().unwrap().to_string(), "a runtime worker panicked");
    }

    #[test]
    fn success_does_not_publish_shutdown_cause() {
        let (commands, _command_rx) = asynchronous::command_channel();
        let shutdown = Shutdown::default();
        let current = ThreadBuilder::default().build(thread::current().id());
        let blocking_worker = blocking::Worker::new(blocking::Pool::new(RuntimeName::default(), None, None), shutdown.clone());
        let endpoint = crate::runtime::testing::worker_endpoint(commands, Waker::noop().clone(), current, blocking_worker);
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(endpoint), shutdown.clone()));

        report_failure(&Ok(()), &dispatcher);

        assert!(shutdown.cause().is_none());
    }
}
