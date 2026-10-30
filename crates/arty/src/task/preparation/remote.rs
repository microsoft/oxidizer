// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

use arty_executor::{TaskOptions, TaskPlacement, TaskSet};
use observed::context::Transfer;
use plurality::{Box as PoolBox, MultiPool, coerce};

use crate::task::Context;
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::{DeferredTask, discard_panicking_drop};

thread_local! {
    static FUTURE_FACTORIES: MultiPool = MultiPool::new();
}

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

/// A future factory for a remote future scheduled from a different thread. The future factory
/// itself must be `Send` to deliver it to the thread where the task is to be scheduled but this
/// does not set any constraints on the future it creates - it may be single-threaded.
///
/// The factory occupies a reusable slot in the submitting thread's pool. Its owning handle can
/// cross threads and outlive that pool, without depending on the submitting thread's stack.
///
/// The factory registers the future it creates with the provided task set instead of returning it.
/// This keeps the concrete future type visible at the point of registration, so the executor can
/// store the future inline in its task storage pool. Returning the future would require erasing its
/// type behind a `Pin<Box<dyn Future>>`, adding a heap allocation to every remote spawn.
type ErasedRemoteFutureFactory = dyn Fn(Context, &TaskSet) + Send;

pub(crate) struct RemoteFutureFactory {
    inner: PoolBox<ErasedRemoteFutureFactory>,
}

#[must_use]
pub(crate) struct RegisteredRemoteTask;

impl RemoteFutureFactory {
    pub(crate) fn new(factory: impl FnOnce(Context, &TaskSet) + Send + 'static) -> Self {
        let factory = Cell::new(Some(factory));
        // A pooled box cannot move an erased FnOnce out of its slot. Take the concrete
        // closure through Cell instead, without requiring its captures to be Unpin.
        let factory = move |cx: Context, tasks: &TaskSet| {
            factory.take().expect("a remote factory is registered exactly once")(cx, tasks);
        };
        let mut factory = Some(factory);
        let inner = FUTURE_FACTORIES
            .try_with(|pool| {
                let factory = factory.take().expect("the pooled allocation consumes the factory once");
                PoolBox::unsize(pool.alloc_box(factory), coerce!(dyn Fn(Context, &TaskSet) + Send))
            })
            .unwrap_or_else(|_| {
                let pool = MultiPool::new();
                let factory = factory.take().expect("the fallback allocation consumes the factory once");
                PoolBox::unsize(pool.alloc_box(factory), coerce!(dyn Fn(Context, &TaskSet) + Send))
            });
        Self { inner }
    }

    pub(crate) fn register(self, cx: Context, tasks: &TaskSet) -> RegisteredRemoteTask {
        (self.inner)(cx, tasks);
        RegisteredRemoteTask
    }
}

pub(crate) fn prepare_remote<FF, F, R>(
    result_pool: &JoinHandlePool,
    future_factory: FF,
    parent_task_enrichment: Transfer,
    placement: TaskPlacement,
) -> (RemoteFutureFactory, JoinHandle<R>)
where
    FF: FnOnce(Context) -> F + Send + 'static,
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    let (handle, result_tx) = result_pool.handle();
    let future_factory = TaskFactory::new(future_factory, parent_task_enrichment);
    let future_factory = RemoteFutureFactory::new(move |cx, tasks| {
        let (future_factory, parent_task_enrichment) = future_factory.into_parts();
        let inner = DeferredTask::new(future_factory, cx);
        let options = TaskOptions::observed(parent_task_enrichment, placement);
        tasks.add(inner, options, result_tx);
    });
    (future_factory, handle)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use observed::Sink;

    use super::*;

    thread_local! {
        static FACTORY_ON_DROP: RefCell<Option<FactoryOnDrop>> = const { RefCell::new(None) };
    }

    struct FactoryOnDrop(mpsc::Sender<()>);

    impl Drop for FactoryOnDrop {
        fn drop(&mut self) {
            drop(RemoteFutureFactory::new(|_, _| {}));
            self.0.send(()).unwrap();
        }
    }

    #[test]
    fn remote_factory_reuses_its_pool_slot() {
        fn factory(_: Context, _: &TaskSet) {}

        let first = RemoteFutureFactory::new(factory);
        let address = std::ptr::from_ref(&*first.inner).cast::<()>();
        let chunks = FUTURE_FACTORIES.with(MultiPool::chunks_allocated);
        drop(first);
        let second = RemoteFutureFactory::new(factory);

        assert_eq!(std::ptr::from_ref(&*second.inner).cast::<()>(), address);
        assert_eq!(FUTURE_FACTORIES.with(MultiPool::chunks_allocated), chunks);
    }

    #[test]
    fn remote_factory_falls_back_during_thread_local_destruction() {
        let (completed, observed) = mpsc::channel();
        std::thread::spawn(move || {
            FACTORY_ON_DROP.with(|slot| {
                assert!(slot.replace(Some(FactoryOnDrop(completed))).is_none());
            });
            FUTURE_FACTORIES.with(|_| {});
        })
        .join()
        .unwrap();

        observed.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
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
