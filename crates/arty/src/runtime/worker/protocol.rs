// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::discard_task_panic_payload;

use crate::task::Builtins;
use crate::task::execution::BoxedRemoteFutureFactory;

pub(crate) type IoAccess = Box<dyn FnOnce(&Builtins) + Send>;

/// Owns a complete submission closure until the destination worker registers it.
///
/// The closure includes every unregistered capture, such as the user's factory and its result
/// sender. Dropping the wrapper contains a panic from disposing that complete closure; successful
/// registration transfers the closure out. Captured endpoints retain their own disposal guards for
/// paths that do not pass through this wrapper.
pub(crate) struct QueuedTaskFactory {
    inner: Option<BoxedRemoteFutureFactory>,
}

impl QueuedTaskFactory {
    pub(crate) fn new(inner: BoxedRemoteFutureFactory) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn into_inner(mut self) -> BoxedRemoteFutureFactory {
        self.inner.take().expect("a queued task factory is consumed exactly once")
    }
}

impl Drop for QueuedTaskFactory {
    fn drop(&mut self) {
        let Some(factory) = self.inner.take() else {
            return;
        };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(factory))) {
            discard_task_panic_payload(panic);
        }
    }
}

/// Owns a synchronous I/O access closure until the destination worker invokes it.
pub(crate) struct QueuedIoAccess {
    inner: Option<IoAccess>,
}

impl QueuedIoAccess {
    pub(crate) fn new(inner: IoAccess) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn into_inner(mut self) -> IoAccess {
        self.inner.take().expect("a queued I/O access is consumed exactly once")
    }
}

impl Drop for QueuedIoAccess {
    fn drop(&mut self) {
        let Some(access) = self.inner.take() else {
            return;
        };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(access))) {
            discard_task_panic_payload(panic);
        }
    }
}

pub(crate) enum AsyncWorkerCommand {
    /// Schedules a new task for execution on this worker, providing the factory function that will
    /// be used to create the future that becomes the body of the task. The factory registers the
    /// future it creates with the worker's task set, which lets the executor store the future
    /// inline instead of behind an extra box.
    ///
    /// The task may end up never getting executed if the runtime is shut down before it gets to it.
    EnqueueTask { future_factory: QueuedTaskFactory },

    /// Executes synchronous I/O context access on this worker.
    AccessIo { access: QueuedIoAccess },

    /// Initiates the shutdown process. The worker will stop accepting new tasks and will discard
    /// any tasks that are enqueued after this command is received (e.g. because other threads do
    /// not yet know about the shutdown).
    ///
    /// It is fine to send this command multiple times - duplicates will be ignored.
    Shutdown,
}

impl fmt::Debug for AsyncWorkerCommand {
    #[cfg_attr(coverage_nightly, coverage(off))] // Command names are diagnostic text only.
    #[cfg_attr(test, mutants::skip)] // We have no contract to test here - can return anything.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnqueueTask { .. } => write!(f, "EnqueueTask"),
            Self::AccessIo { .. } => write!(f, "AccessIo"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    use arty_executor::{Executor, TaskPlacement};
    use observed::Sink;

    use super::*;
    use crate::task::execution::prepare_remote;

    struct PanicWake(AtomicBool);

    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Release);
            panic!("queued join cancellation notification");
        }
    }

    struct PanicOnDrop(Arc<AtomicUsize>);

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
            panic!("captured factory destructor panic");
        }
    }

    #[test]
    fn queued_factory_into_inner_returns_the_factory() {
        let runtime = crate::runtime::Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let builtins = runtime.scheduler().block_on(async |builtins| builtins).unwrap();
        let executor = Executor::builder().build();
        let invoked = Arc::new(AtomicBool::new(false));
        let factory_invoked = Arc::clone(&invoked);
        let factory: BoxedRemoteFutureFactory = Box::new(move |_, _| {
            factory_invoked.store(true, Ordering::Release);
        });

        QueuedTaskFactory::new(factory).into_inner()(builtins, &executor.tasks());

        assert!(invoked.load(Ordering::Acquire));
        runtime.stop().unwrap();
    }

    #[test]
    fn queued_factory_disposal_contains_join_notification_panic() {
        let sink = Sink::noop();
        let result_pool = arty_executor::remote::Pool::new();
        let (factory, join) = prepare_remote(
            &result_pool,
            |_| async { 42 },
            sink.transfer_context(),
            TaskPlacement::Any,
            performables::arc::Arc::new(AtomicBool::new(false)),
        );
        let mut join = pin!(join);
        let wake = Arc::new(PanicWake(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&wake));
        assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(QueuedTaskFactory::new(factory)))).unwrap();

        let Poll::Ready(Err(error)) = join.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("discarded queued task must report shutdown");
        };
        assert!(error.is_shutdown());
        assert!(wake.0.load(Ordering::Acquire));
    }

    #[test]
    fn queued_factory_disposal_contains_captured_destructor_panic() {
        let drops = Arc::new(AtomicUsize::new(0));
        let capture = PanicOnDrop(Arc::clone(&drops));
        let factory: BoxedRemoteFutureFactory = Box::new(move |_, _| {
            let _ = &capture;
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(QueuedTaskFactory::new(factory));
        }));

        result.unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn queued_io_access_disposal_contains_captured_destructor_panic() {
        let drops = Arc::new(AtomicUsize::new(0));
        let capture = PanicOnDrop(Arc::clone(&drops));
        let access: IoAccess = Box::new(move |_| {
            let _ = &capture;
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(QueuedIoAccess::new(access));
        }));

        result.unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
