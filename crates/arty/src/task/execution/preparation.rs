// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::pending;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

use arty_executor::{RemoteJoinHandle, RemoteTaskSender, TaskOptions, TaskPlacement, TaskSet, discard_task_panic_payload};
use observed::context::Transfer;
use performables::arc::Arc;

use crate::task::Builtins;
use crate::task::join::JoinHandle;

struct TaskFactory<F> {
    pending: Option<(F, Transfer)>,
}

impl<F> TaskFactory<F> {
    fn new(inner: F, enrichment: Transfer) -> Self {
        Self {
            pending: Some((inner, enrichment)),
        }
    }

    fn into_parts(mut self) -> (F, Transfer) {
        self.pending.take().expect("a queued factory is consumed exactly once")
    }
}

impl<F> Drop for TaskFactory<F> {
    fn drop(&mut self) {
        let Some((factory, enrichment)) = self.pending.take() else {
            return;
        };
        let _guard = enrichment.apply_current_thread();
        discard_panicking_drop(factory);
    }
}

fn discard_panicking_drop<T>(value: T) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(value))) {
        discard_task_panic_payload(panic);
    }
}

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
pub(crate) type BoxedRemoteFutureFactory = Box<dyn FnOnce(Builtins, &TaskSet) + Send + 'static>;

pub(crate) fn prepare_remote<FF, F, R>(
    future_factory: FF,
    parent_task_enrichment: Transfer,
    placement: TaskPlacement,
    shutdown_signal: Arc<AtomicBool>,
) -> (BoxedRemoteFutureFactory, JoinHandle<R>)
where
    FF: FnOnce(Builtins) -> F + Send + 'static,
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    let (result_tx, result_rx) = RemoteJoinHandle::channel();
    let future_factory = TaskFactory::new(future_factory, parent_task_enrichment);
    let future_factory: BoxedRemoteFutureFactory = Box::new(move |cx, tasks| {
        // Factory invocation belongs inside the same panic boundary as polling.
        let (future_factory, parent_task_enrichment) = future_factory.into_parts();
        let inner = async move {
            if shutdown_signal.load(Ordering::Acquire) {
                return pending().await;
            }
            future_factory(cx).await
        };

        // The executor join handle is not used - the task delivers its result through the
        // channel above, which unlike the executor's join handle can cross thread boundaries.
        let options = TaskOptions::observed(parent_task_enrichment, placement);
        drop(tasks.add_with_remote_result(inner, options, result_tx));
    });
    (future_factory, JoinHandle::new(result_rx))
}

pub(crate) fn prepare_blocking<F, R>(body: F) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (result_tx, result_rx) = RemoteJoinHandle::channel();
    let task = BlockingTask {
        pending: Some((body, result_tx)),
    };
    (move || task.run(), JoinHandle::new(result_rx))
}

struct BlockingTask<F, R: Send + 'static> {
    pending: Option<(F, RemoteTaskSender<R>)>,
}

impl<F, R> BlockingTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn run(mut self) {
        let (body, result_tx) = self.pending.take().expect("a blocking task is consumed exactly once");
        let delivery = match catch_unwind(AssertUnwindSafe(body)) {
            Ok(result) => result_tx.completed(result),
            Err(panic) => result_tx.panicked(panic),
        };
        if let Err(panic) = delivery {
            discard_task_panic_payload(panic);
        }
    }
}

impl<F, R: Send + 'static> Drop for BlockingTask<F, R> {
    fn drop(&mut self) {
        let Some((body, result_tx)) = self.pending.take() else {
            return;
        };
        discard_panicking_drop(body);
        discard_panicking_drop(result_tx);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::panic::panic_any;
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    use arty_executor::{CycleOutcome, Executor};
    use futures::executor::block_on;
    use observed::Sink;

    use super::*;
    use crate::runtime::{Runtime, WorkersPolicy};

    fn test_builtins() -> (Runtime, Builtins) {
        let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
        (runtime, builtins)
    }

    #[test]
    fn remote_factory_remains_deferred_until_poll() {
        let invoked = Arc::new(AtomicBool::new(false));
        let factory_invoked = Arc::clone(&invoked);
        let sink = Sink::noop();
        let (factory, handle) = prepare_remote(
            move |_| {
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
            TaskPlacement::Any,
            performables::arc::Arc::new(AtomicBool::new(false)),
        );

        let executor = Executor::builder().owner_waker(Waker::noop().clone()).build();
        let (_runtime, builtins) = test_builtins();
        factory(builtins, &executor.tasks());
        let before_poll = invoked.load(Ordering::Relaxed);
        run_to_completion(&executor);
        let result = block_on(handle).unwrap();

        assert_eq!((before_poll, invoked.load(Ordering::Relaxed), result), (false, true, 42),);
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
        let (factory, handle) = prepare_remote(
            |_| -> std::future::Ready<()> { panic_any(Payload(42)) },
            sink.transfer_context(),
            TaskPlacement::Any,
            performables::arc::Arc::new(AtomicBool::new(false)),
        );
        let executor = Executor::builder().owner_waker(Waker::noop().clone()).build();
        let (_runtime, builtins) = test_builtins();
        factory(builtins, &executor.tasks());
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

    #[test]
    fn queued_factory_disposes_a_panicking_factory_without_unwinding() {
        let dropped = AtomicBool::new(false);
        let result = catch_unwind(AssertUnwindSafe(|| {
            struct PanicOnDrop<'a>(&'a AtomicBool);

            impl Drop for PanicOnDrop<'_> {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Release);
                    panic!("factory drop");
                }
            }

            let sink = Sink::noop();
            drop(TaskFactory::new(PanicOnDrop(&dropped), sink.transfer_context()));
        }));
        result.unwrap();
        assert!(dropped.load(Ordering::Acquire));
    }
}
