// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::TaskSet;
use events_once::LocalEvent;
use observed::Sink;
use observed::context::Transfer;
use performables::sync::channel::oneshot;

use crate::task::execution::local::LocalTaskFuture;
use crate::task::execution::remote::RemoteTaskFuture;
use crate::task::execution::result::TaskResult;
use crate::task::join::{JoinHandle, LocalJoinHandle};

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

pub(in crate::task) fn prepare_local<F, R>(
    future: F,
    parent_task_enrichment: Transfer,
    sink: Sink,
) -> (impl Future<Output = ()>, LocalJoinHandle<R>)
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    let (result_tx, result_rx) = LocalEvent::boxed();
    // Drop a completed future inside the polling panic boundary, before publishing its result.
    let inner = async move { future.await };
    let future = LocalTaskFuture::new(inner, result_tx, parent_task_enrichment, sink);
    (future, LocalJoinHandle::new(result_rx))
}

pub(crate) fn prepare_remote<C, FF, F, R>(
    future_factory: FF,
    parent_task_enrichment: Transfer,
    sink: Sink,
) -> (BoxedRemoteFutureFactory<C>, JoinHandle<R>)
where
    C: 'static,
    FF: FnOnce(C) -> F + Send + 'static,
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    let (result_tx, result_rx) = oneshot::<TaskResult<R>>();
    let future_factory: BoxedRemoteFutureFactory<C> = Box::new(move |cx, tasks| {
        // Factory invocation belongs inside the same panic boundary as polling.
        let inner = async move { future_factory(cx).await };

        // The executor join handle is not used - the task delivers its result through the
        // channel above, which unlike the executor's join handle can cross thread boundaries.
        drop(tasks.add(RemoteTaskFuture::new(inner, result_tx, parent_task_enrichment, sink)));
    });
    (future_factory, JoinHandle::new(result_rx))
}

pub(crate) fn prepare_blocking<F, R>(body: F) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (result_tx, result_rx) = oneshot::<TaskResult<R>>();
    let task = move || {
        // We AssertUnwindSafe here because we consider the task completed on panic.
        // Whatever it did to its internal state is now irrelevant and if it corrupted
        // some shared state, that is not really something we can do anything about
        // (a conscientious service will abort on panic to avoid that).
        let body_result = catch_unwind(AssertUnwindSafe(body));

        match body_result {
            Ok(result) => drop(result_tx.send(TaskResult::Completed(result))),
            Err(panic) => {
                drop(result_tx.send(TaskResult::Panicked(panic)));
            }
        }
    };
    (task, JoinHandle::new(result_rx))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::cell::Cell;
    use std::future::pending;
    use std::panic::panic_any;
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};

    use arty_executor::testing::new_guarded_executor;
    use arty_executor::{CycleOutcome, Executor};
    use futures::executor::block_on;
    use futures::future::join;

    use super::*;

    #[test]
    fn remote_factory_remains_deferred_until_poll() {
        let invoked = Arc::new(AtomicBool::new(false));
        let factory_invoked = Arc::clone(&invoked);
        let sink = Sink::noop();
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
        );

        let executor = new_guarded_executor(Waker::noop().clone());
        factory((), &executor.tasks());
        let before_poll = invoked.load(Ordering::Relaxed);
        run_to_completion(&executor);
        let result = block_on(handle).unwrap();

        assert_eq!((before_poll, invoked.load(Ordering::Relaxed), result), (false, true, 42),);
    }

    /// Drives the executor until it reports that no further progress can be made.
    fn run_to_completion(executor: &Executor) {
        while executor.execute_cycle() == CycleOutcome::Continue {}
    }

    #[test]
    fn local_preparation_preserves_non_send_results() {
        let value = Rc::new(Cell::new(42));
        let expected = Rc::clone(&value);
        let sink = Sink::noop();
        let (task, handle) = prepare_local(async move { value }, sink.transfer_context(), sink);
        let ((), actual) = block_on(join(task, handle));

        assert!(Rc::ptr_eq(&actual.unwrap(), &expected));
    }

    #[test]
    fn local_preparation_reports_a_panic_without_unwinding_the_joiner() {
        #[derive(Debug, Eq, PartialEq)]
        struct PanicPayload(u32);

        let sink = Sink::noop();
        let (task, handle) = prepare_local::<_, ()>(async { panic_any(PanicPayload(42)) }, sink.transfer_context(), sink);
        block_on(task);

        let outcome = catch_unwind(AssertUnwindSafe(|| block_on(handle))).unwrap();
        assert!(outcome.unwrap_err().is_panic());
    }

    #[test]
    fn discarded_local_preparation_reports_shutdown() {
        let sink = Sink::noop();
        let (task, handle) = prepare_local(pending::<()>(), sink.transfer_context(), sink);
        drop(task);
        let mut handle = pin!(handle);
        let mut context = Context::from_waker(Waker::noop());

        let Poll::Ready(Err(error)) = handle.as_mut().poll(&mut context) else {
            panic!("discarded local work must report shutdown");
        };
        assert!(error.is_shutdown());
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
            |()| -> std::future::Ready<()> { panic_any(Payload(42)) },
            sink.transfer_context(),
            sink,
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
}
