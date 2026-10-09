// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;

use arty_executor::TaskSet;
use events_once::{BoxedSender, Event};
use observed::Sink;
use observed::context::Transfer;
use performables::arc::Arc;

use crate::runtime::seismograph::TaskTelemetryPlacement;
use crate::task::Builtins;
use crate::task::execution::discard_panic;
use crate::task::execution::remote::RemoteTaskFuture;
use crate::task::execution::result::TaskResult;
use crate::task::execution::storage::TaskFactory;
use crate::task::join::JoinHandle;

/// A future factory for a remote future scheduled from a different thread. The future factory
/// itself must be `Send` to deliver it to the thread where the task is to be scheduled but this
/// does not set any constraints on the future it creates - it may be single-threaded.
///
/// The future factory is boxed up for transit between threads and has 'static to signal that it has
/// no dependency on the stack of any specific thread.
///
/// The factory registers the future it creates with the provided task set instead of returning it.
/// This keeps the concrete future type visible at the point of registration, so the executor can
/// store the future inline in its task storage pool. Returning the future would require erasing its
/// type behind a `Pin<Box<dyn Future>>`, adding a heap allocation to every remote spawn.
pub(crate) type BoxedRemoteFutureFactory<FgArg> = Box<dyn FnOnce(FgArg, &TaskSet) + Send + 'static>;

pub(crate) fn prepare_remote<C, FF, F, R>(
    future_factory: FF,
    parent_task_enrichment: Transfer,
    sink: Sink,
    shutdown_signal: Arc<AtomicBool>,
    task_telemetry: TaskTelemetryPlacement,
) -> (BoxedRemoteFutureFactory<C>, JoinHandle<R>)
where
    C: 'static,
    FF: FnOnce(C) -> F + Send + 'static,
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    let (result_tx, result_rx) = Event::<TaskResult<R>>::boxed();
    let future_factory = TaskFactory::new(future_factory, parent_task_enrichment, sink);
    let future_factory: BoxedRemoteFutureFactory<C> = Box::new(move |cx, tasks| {
        let task_telemetry = task_telemetry.materialized();
        // Factory invocation belongs inside the same panic boundary as polling.
        let (future_factory, parent_task_enrichment, sink) = future_factory.into_parts();
        let inner = async move { future_factory(cx).await };

        // The executor join handle is not used - the task delivers its result through the
        // channel above, which unlike the executor's join handle can cross thread boundaries.
        drop(tasks.add(RemoteTaskFuture::new_with_shutdown(
            inner,
            result_tx,
            parent_task_enrichment,
            sink,
            Some(shutdown_signal),
            task_telemetry,
        )));
    });
    (future_factory, JoinHandle::new(result_rx))
}

pub(crate) fn prepare_remote_on_worker<FF, F, R>(
    future_factory: FF,
    builtins: Builtins,
    parent_task_enrichment: Transfer,
    sink: Sink,
    shutdown_signal: Arc<AtomicBool>,
    tasks: &TaskSet,
    task_telemetry: TaskTelemetryPlacement,
) -> JoinHandle<R>
where
    FF: FnOnce(Builtins) -> F + Send + 'static,
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    let task_telemetry = task_telemetry.materialized();
    let (result_tx, result_rx) = Event::<TaskResult<R>>::boxed();
    let inner = async move { future_factory(builtins).await };
    drop(tasks.add(RemoteTaskFuture::new_with_shutdown(
        inner,
        result_tx,
        parent_task_enrichment,
        sink,
        Some(shutdown_signal),
        task_telemetry,
    )));
    JoinHandle::new(result_rx)
}

pub(crate) fn prepare_blocking<F, R>(body: F) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (result_tx, result_rx) = Event::<TaskResult<R>>::boxed();
    let task = BlockingTask {
        body: Some(body),
        result_tx: Some(result_tx),
    };
    (move || task.run(), JoinHandle::new(result_rx))
}

struct BlockingTask<F, R: Send + 'static> {
    body: Option<F>,
    result_tx: Option<BoxedSender<TaskResult<R>>>,
}

impl<F, R> BlockingTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn run(mut self) {
        let body = self.body.take().expect("blocking task body is consumed exactly once");
        let result_tx = self.result_tx.take().expect("blocking task sender is consumed exactly once");
        // We AssertUnwindSafe here because we consider the task completed on panic.
        // Whatever it did to its internal state is now irrelevant and if it corrupted
        // some shared state, that is not really something we can do anything about
        // (a conscientious service will abort on panic to avoid that).
        let body_result = catch_unwind(AssertUnwindSafe(body));

        match body_result {
            Ok(result) => {
                if let Err(panic) = catch_unwind(AssertUnwindSafe(|| result_tx.send(TaskResult::Completed(result)))) {
                    discard_panic(panic);
                }
            }
            Err(panic) => {
                if let Err(disposal) = catch_unwind(AssertUnwindSafe(|| result_tx.send(TaskResult::Panicked(panic)))) {
                    discard_panic(disposal);
                }
            }
        }
    }
}

impl<F, R: Send + 'static> Drop for BlockingTask<F, R> {
    fn drop(&mut self) {
        if let Some(body) = self.body.take()
            && let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(body)))
        {
            discard_panic(panic);
        }
        if let Some(sender) = self.result_tx.take()
            && let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(sender)))
        {
            discard_panic(panic);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::panic::panic_any;
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::task::{Context, Poll, Wake, Waker};

    use arty_executor::testing::new_guarded_executor;
    use arty_executor::{CycleOutcome, Executor};
    use futures::executor::block_on;
    use performables::arc::Arc as PArc;

    use super::*;
    use crate::runtime::seismograph::RuntimeTelemetry;

    fn telemetry() -> (RuntimeTelemetry, TaskTelemetryPlacement) {
        let (runtime, _workers) = RuntimeTelemetry::register(0..1);
        let (task, _enqueued) = runtime.register_task::<()>(0).into_parts();
        (runtime, task)
    }

    #[test]
    fn remote_factory_remains_deferred_until_poll() {
        let invoked = Arc::new(AtomicBool::new(false));
        let factory_invoked = Arc::clone(&invoked);
        let sink = Sink::noop();
        let shutdown_signal = PArc::new(AtomicBool::new(false));
        let (_telemetry, task_telemetry) = telemetry();
        let (factory, handle) = prepare_remote(
            move |()| {
                // This test polls and observes the factory on one thread; no synchronization is needed.
                factory_invoked.store(true, Ordering::Relaxed);
                async {
                    // Retain an Rc across await so the factory-produced future remains non-Send.
                    let value = Rc::new(42);
                    std::future::ready(()).await;
                    *value
                }
            },
            sink.transfer_context(),
            sink,
            shutdown_signal,
            task_telemetry,
        );

        let executor = new_guarded_executor(Waker::noop().clone());
        factory((), &executor.tasks());
        let before_poll = invoked.load(Ordering::Relaxed);
        run_to_completion(&executor);
        let result = block_on(handle).unwrap();

        assert_eq!((before_poll, invoked.load(Ordering::Relaxed), result), (false, true, 42),);
    }

    #[test]
    fn remote_factory_is_not_invoked_if_shutdown_starts_after_registration() {
        let invoked = Arc::new(AtomicBool::new(false));
        let shutdown_signal = PArc::new(AtomicBool::new(false));
        let sink = Sink::noop();
        let (_telemetry, task_telemetry) = telemetry();
        let (factory, handle) = prepare_remote(
            {
                let invoked = Arc::clone(&invoked);
                move |()| {
                    invoked.store(true, Ordering::Relaxed);
                    std::future::ready(())
                }
            },
            sink.transfer_context(),
            sink,
            PArc::clone(&shutdown_signal),
            task_telemetry,
        );
        let executor = new_guarded_executor(Waker::noop().clone());
        factory((), &executor.tasks());

        shutdown_signal.store(true, Ordering::Release);
        run_to_completion(&executor);

        assert!(!invoked.load(Ordering::Relaxed));
        assert!(block_on(handle).unwrap_err().is_shutdown());
    }

    /// Drives the executor until it reports that no further progress can be made.
    fn run_to_completion(executor: &Executor) {
        while executor.execute_cycle() == CycleOutcome::Continue {}
    }

    struct PanicWake;

    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("join receiver notification");
        }
    }

    #[test]
    fn blocking_task_delivers_its_result() {
        let (task, handle) = prepare_blocking(|| 42);
        task();

        assert_eq!(block_on(handle).unwrap(), 42);
    }

    #[test]
    fn blocking_panic_is_delivered_to_joiner() {
        let (task, handle) = prepare_blocking(|| panic!("blocking task"));
        task();

        assert!(block_on(handle).unwrap_err().is_panic());
    }

    #[test]
    fn remote_factory_panic_is_delivered_to_joiner() {
        #[derive(Debug, PartialEq)]
        struct Payload(u32);

        let sink = Sink::noop();
        let shutdown_signal = PArc::new(AtomicBool::new(false));
        let (_telemetry, task_telemetry) = telemetry();
        let (factory, handle) = prepare_remote(
            |()| -> std::future::Ready<()> { panic_any(Payload(42)) },
            sink.transfer_context(),
            sink,
            shutdown_signal,
            task_telemetry,
        );
        let executor = new_guarded_executor(Waker::noop().clone());
        factory((), &executor.tasks());
        run_to_completion(&executor);
        assert!(block_on(handle).unwrap_err().is_panic());
    }

    #[test]
    fn discarded_preparation_reports_shutdown() {
        let (task, handle) = prepare_blocking(|| 42);
        drop(task);
        let mut handle = pin!(handle);
        let mut context = Context::from_waker(Waker::noop());

        let Poll::Ready(Err(error)) = handle.as_mut().poll(&mut context) else {
            panic!("discarded work must report shutdown");
        };
        assert!(error.is_shutdown());
    }

    #[test]
    fn discarded_blocking_task_contains_body_destructor_panic() {
        struct PanicOnDrop;

        impl Drop for PanicOnDrop {
            fn drop(&mut self) {
                panic!("discarded blocking capture");
            }
        }

        let capture = PanicOnDrop;
        let (task, handle) = prepare_blocking(move || drop(capture));
        catch_unwind(AssertUnwindSafe(|| drop(task))).unwrap();
        assert!(block_on(handle).unwrap_err().is_shutdown());
    }

    #[test]
    fn discarded_blocking_task_contains_join_waker_panic() {
        let (task, handle) = prepare_blocking(|| 42);
        let mut handle = pin!(handle);
        let waker = Waker::from(Arc::new(PanicWake));
        assert!(handle.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

        catch_unwind(AssertUnwindSafe(|| drop(task))).unwrap();

        let Poll::Ready(Err(error)) = handle.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("discarded blocking work must report shutdown");
        };
        assert!(error.is_shutdown());
    }
}
