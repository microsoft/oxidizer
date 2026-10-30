// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::thread::{self, JoinHandle};

use arty_executor::discard_task_panic_payload;
use arty_io_host::IoRuntime;
use many_cpus::ProcessorSet;
use observed::{Sink, emit};
use performables::arc::Arc;
use performables::sync::channel;
use performables::sync::channel::{OneshotReceiver, OneshotSender, Sender};
use thread_aware::{Thread as ThreadCoordinate, ThreadAware, ThreadBuilder};
use tick::runtime::InactiveClock;

use crate::io::{Io, RuntimeDriverHost};
use crate::runtime::PanicPolicy;
use crate::runtime::config::MonitorOptions;
use crate::runtime::dispatch::{DispatcherClient, WorkerEndpoint};
use crate::runtime::error::Error;
use crate::runtime::shutdown::Shutdown;
use crate::runtime::telemetry::events::{
    AsyncWorkerActive, AsyncWorkerStarted, AsyncWorkerStopped, PanicMessage, SystemMetricCount, ThreadExiting, ThreadName, ThreadPanicked,
    ThreadSpawn, ThreadStarted,
};
use crate::runtime::worker::{asynchronous, blocking};

/// Inputs required to spawn one asynchronous worker.
#[derive(Debug)]
pub(crate) struct StartOptions {
    pub(crate) stack_size: usize,
    pub(crate) inactive_clock: InactiveClock,
    pub(crate) processor: ProcessorSet,
    pub(crate) worker_index: usize,
    pub(crate) thread_builder: ThreadBuilder,
    pub(crate) blocking_pools: blocking::Pools,
    pub(crate) shutdown: Shutdown,
    pub(crate) io_runtime: IoRuntime,
    pub(crate) sink: Sink,
    pub(crate) monitor_options: MonitorOptions,
    pub(crate) panic_policy: PanicPolicy,
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
    let (command_tx, command_rx) = channel::unbounded();
    let (endpoint_tx, endpoint_rx) = channel::unbounded();
    let (activation_tx, activation_rx) = channel::oneshot();
    let (ready_tx, ready_rx) = channel::unbounded();
    let thread_name = format!("oxidizer-rt-{}", options.worker_index);
    let stack_size = options.stack_size;
    let sink = options.sink.clone();
    let thread_entry = ThreadEntry {
        options,
        command_rx,
        activation_rx,
        ready_tx,
        endpoint_tx,
    };
    let join_handle = spawn_thread(&sink, &thread_name, stack_size, move || thread_entry.run());
    let endpoint = endpoint_rx
        .recv()
        .expect("failed to receive the worker endpoint from the starting worker");

    SpawnedWorker {
        endpoint: WorkerEndpoint {
            command_tx,
            waker: endpoint.waker,
            thread: endpoint.thread,
            blocking_worker: endpoint.blocking_worker,
        },
        activation_tx,
        ready_rx,
        join_handle,
    }
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
    command_rx: channel::Receiver<asynchronous::Command>,
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
            sink,
            monitor_options,
            panic_policy,
            ..
        } = options;

        let worker_sink = sink.clone();
        let thread_builder = thread_builder.with_numa_node(processor.processors().first().memory_region_id());
        processor.pin_current_thread_to();
        let current = thread_builder.build(thread::current().id());
        asynchronous::LocalWorker::attach();

        let mut clock = inactive_clock;
        clock.relocate(None, &current);

        let signal = Arc::new(asynchronous::Signal::default());
        let worker_waker = asynchronous::Signal::waker(&signal);
        let blocking_panic_policy = match panic_policy.clone().into_executor_policy() {
            arty_executor::PanicPolicy::Ignore => blocking::PanicPolicy::Ignore,
            arty_executor::PanicPolicy::Shutdown => blocking::PanicPolicy::Shutdown(worker_waker.clone()),
        };
        let blocking_worker = blocking::Worker::new(
            blocking_pools.pool_for_worker(),
            worker_sink.clone(),
            shutdown,
            blocking_panic_policy,
        );

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
        signal.set_driver_waker(driver_waker);

        let mut worker = asynchronous::Worker::new(
            command_rx,
            dispatcher.clone(),
            current,
            sink,
            io,
            &blocking_worker,
            clock,
            signal,
            monitor_options,
            panic_policy,
        );

        emit!(
            worker_sink,
            AsyncWorkerStarted {
                processor_index: worker_index.into(),
            }
        );
        emit!(worker_sink, AsyncWorkerActive { delta: 1 });
        _ = ready_tx.send(());

        let run_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run()));
        report_failure(&run_outcome, &dispatcher);
        let drop_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(worker)));
        report_failure(&drop_outcome, &dispatcher);
        let outcome = combine_outcomes(run_outcome, drop_outcome);
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

fn spawn_thread(sink: &Sink, name: &str, stack_size: usize, task: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    let owner_thread = thread::current();
    emit!(
        sink,
        ThreadSpawn {
            name: ThreadName::new(name),
            stack_size_bytes: Some(SystemMetricCount::from(stack_size)),
            owner_name: owner_thread.name().map(ThreadName::new),
            owner_id: owner_thread.id().into(),
        }
    );
    let thread_sink = sink.clone();

    thread::Builder::new()
        .name(name.to_string())
        .stack_size(stack_size)
        .spawn(move || thread_main(&thread_sink, task))
        // Fallible construction and partial-startup cleanup are intentionally
        // deferred: https://github.com/microsoft/oxidizer/pull/785#discussion_r4183811801
        .expect("failed to spawn thread")
}

fn thread_main(sink: &Sink, task: impl FnOnce() + Send + 'static) {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let thread = thread::current();
    emit!(
        sink,
        ThreadStarted {
            name: thread.name().map(ThreadName::new),
            id: thread.id().into(),
        }
    );

    let result = catch_unwind(AssertUnwindSafe(task));
    if let Err(panic) = result {
        let message = if let Some(message) = panic.downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = panic.downcast_ref::<String>() {
            message.clone()
        } else {
            "unknown panic".to_owned()
        };
        emit!(
            sink,
            ThreadPanicked {
                name: thread.name().map(ThreadName::new),
                id: thread.id().into(),
                message: PanicMessage(message),
            }
        );
        std::panic::resume_unwind(panic);
    }

    emit!(
        sink,
        ThreadExiting {
            name: thread.name().map(ThreadName::new),
            id: thread.id().into(),
        }
    );
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
mod tests {
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    use nonempty::NonEmpty;
    use observed::Value;
    use observed_testing::{TEST_ID, test_emitter};

    use super::*;
    use crate::runtime::dispatch::DispatcherCore;

    #[test]
    fn spawn_executes_task_successfully() {
        let result = StdArc::new(StdMutex::new(0));
        let handle = spawn_thread(&Sink::noop(), "test-thread", 2 * 1024 * 1024, {
            let result = StdArc::clone(&result);
            move || *result.lock().unwrap() = 42
        });

        handle.join().unwrap();

        assert_eq!(*result.lock().unwrap(), 42);
    }

    #[test]
    fn spawn_propagates_panic() {
        let handle = spawn_thread(&Sink::noop(), "panicking-thread", 2 * 1024 * 1024, || {
            panic!("task panicked");
        });

        handle.join().unwrap_err();
    }

    #[test]
    fn panicking_thread_emits_panic_event() {
        let cases: [(fn(), &str); 3] = [
            (|| panic!("test test 123"), "test test 123"),
            (|| std::panic::panic_any(String::from("owned message")), "owned message"),
            (|| std::panic::panic_any(42u32), "unknown panic"),
        ];
        for (body, expected_message) in cases {
            let (sink, processor) = test_emitter(TEST_ID);
            let handle = spawn_thread(&sink, "panicking-thread", 2 * 1024 * 1024, body);
            handle.join().unwrap_err();
            let panic_event = processor
                .events()
                .into_iter()
                .find(|event| event.name() == "arty.rt.thread.panic")
                .expect("a panicking thread emits arty.rt.thread.panic");
            let attribute = |key: &str| -> Option<String> {
                panic_event
                    .dimensions()
                    .into_iter()
                    .find(|(name, _)| name == key)
                    .and_then(|(_, value)| match value {
                        Value::String(value) => Some(value.to_string()),
                        _ => None,
                    })
            };

            assert_eq!(attribute("panic.message"), Some(expected_message.to_owned()));
            assert_eq!(attribute("thread.name"), Some("panicking-thread".to_owned()));
        }
    }

    #[test]
    fn spawn_names_the_thread() {
        const THREAD_NAME: &str = "named-thread";
        let handle = spawn_thread(&Sink::noop(), THREAD_NAME, 2 * 1024 * 1024, || {
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
        let (command_tx, _command_rx) = channel::unbounded();
        let shutdown = Shutdown::default();
        let current = ThreadBuilder::default().build(thread::current().id());
        let blocking_worker = blocking::Worker::new(
            blocking::Pool::new(None),
            Sink::noop(),
            shutdown.clone(),
            blocking::PanicPolicy::Ignore,
        );
        let endpoint = WorkerEndpoint {
            command_tx,
            waker: Waker::noop().clone(),
            thread: current,
            blocking_worker,
        };
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(endpoint), Sink::noop(), shutdown.clone()));

        report_failure(&Err(Box::new("worker panic")), &dispatcher);

        assert_eq!(shutdown.error().unwrap().to_string(), "a runtime worker panicked");
    }

    #[test]
    fn success_does_not_publish_shutdown_cause() {
        let (command_tx, _command_rx) = channel::unbounded();
        let shutdown = Shutdown::default();
        let current = ThreadBuilder::default().build(thread::current().id());
        let blocking_worker = blocking::Worker::new(
            blocking::Pool::new(None),
            Sink::noop(),
            shutdown.clone(),
            blocking::PanicPolicy::Ignore,
        );
        let endpoint = WorkerEndpoint {
            command_tx,
            waker: Waker::noop().clone(),
            thread: current,
            blocking_worker,
        };
        let dispatcher = DispatcherClient::new(DispatcherCore::new(NonEmpty::new(endpoint), Sink::noop(), shutdown.clone()));

        report_failure(&Ok(()), &dispatcher);

        assert!(shutdown.cause().is_none());
    }
}
