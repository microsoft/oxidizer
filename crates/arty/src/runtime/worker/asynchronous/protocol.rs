// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Commands and queued work sent to an asynchronous worker.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::discard_task_panic_payload;

use crate::task::Context;
use crate::task::preparation::RemoteFutureFactory;

pub(crate) type IoAccess = Box<dyn FnOnce(&Context) + Send>;

/// Owns a queued closure until the destination worker consumes it.
///
/// Dropping the wrapper contains a panic from disposing the closure. Successful
/// consumption transfers the closure out exactly once.
pub(crate) struct QueuedClosure<T> {
    inner: Option<T>,
}

impl<T> QueuedClosure<T> {
    pub(crate) fn new(inner: T) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn into_inner(mut self) -> T {
        self.inner.take().expect("a queued closure is consumed exactly once")
    }
}

impl<T> Drop for QueuedClosure<T> {
    fn drop(&mut self) {
        let Some(closure) = self.inner.take() else {
            return;
        };
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(closure))) {
            discard_task_panic_payload(panic);
        }
    }
}

/// Owns a complete submission closure until the destination worker registers it.
///
/// The closure includes every unregistered capture, such as the user's factory and its result
/// sender. Captured endpoints retain their own disposal guards for paths that do not pass through
/// this wrapper.
pub(crate) type QueuedFactory = QueuedClosure<RemoteFutureFactory>;

/// Owns a synchronous I/O access closure until the destination worker invokes it.
pub(crate) type QueuedIoAccess = QueuedClosure<IoAccess>;

pub(crate) enum Command {
    /// Schedules a new task on this worker.
    ///
    /// The factory creates the task future and registers it with the worker task
    /// set. This lets the executor store the future inline. The task does not
    /// run if shutdown reaches the worker first.
    EnqueueTask { future_factory: QueuedFactory },

    /// Executes synchronous I/O context access on this worker.
    AccessIo { access: QueuedIoAccess },

    /// Starts shutdown on this worker.
    ///
    /// The worker stops accepting new tasks. It discards tasks enqueued after
    /// this command, including tasks from threads that have not observed
    /// shutdown. Duplicate commands have no effect.
    Shutdown,
}

impl fmt::Debug for Command {
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

    use super::*;
    use crate::task::preparation::prepare_remote;

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
        let context = runtime.scheduler().block_on(async |context| context).unwrap();
        let executor = Executor::builder().build();
        let invoked = Arc::new(AtomicBool::new(false));
        let factory_invoked = Arc::clone(&invoked);
        let factory = RemoteFutureFactory::new(move |_, _| {
            factory_invoked.store(true, Ordering::Release);
        });

        let _registered = QueuedFactory::new(factory).into_inner().register(context, &executor.tasks());

        assert!(invoked.load(Ordering::Acquire));
        runtime.stop().unwrap();
    }

    #[test]
    fn queued_factory_disposal_contains_join_notification_panic() {
        let result_pool = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (factory, join) = prepare_remote(&result_pool, |_| async { 42 }, TaskPlacement::Any);
        let mut join = pin!(join);
        let wake = Arc::new(PanicWake(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&wake));
        assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(QueuedFactory::new(factory)))).unwrap();

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
        let factory = RemoteFutureFactory::new(move |_, _| {
            let _ = &capture;
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(QueuedFactory::new(factory));
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
