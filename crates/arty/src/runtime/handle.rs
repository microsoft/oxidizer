// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::builder::RuntimeBuilder;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::error::Error;
use crate::runtime::worker::WorkerJoiner;
use crate::task::RuntimeScheduler;

/// Owns an Arty runtime's workers and their shutdown.
///
/// Use this type to run async work from synchronous code. Construction
/// assigns each async worker to an available processor. Exact worker policies
/// can assign multiple workers to one processor. A task stays on its worker
/// until it finishes or is cancelled.
///
/// Borrow its [`RuntimeScheduler`] through
/// [`scheduler`](Self::scheduler) to submit work or block on
/// tasks that borrow caller-owned data. Consume the owner with [`stop`](Self::stop)
/// when the required work has finished.
///
/// # Drop
///
/// Dropping the owner requests shutdown and normally waits for workers to stop. Pending
/// async tasks and queued blocking callbacks are cancelled; blocking
/// callbacks already running are allowed to finish. A blocking callback that
/// never returns can therefore prevent shutdown from completing.
///
/// Implicit cleanup cannot return shutdown errors. Call [`stop`](Self::stop)
/// to receive errors for worker panics or I/O driver failures during
/// initialization or cycle execution. I/O driver shutdown failures are not returned.
///
/// Dropping the owner on a task or blocking worker panics. Keep the owner on an
/// external thread, and use [`RuntimeControl::request_stop`](crate::runtime::RuntimeControl::request_stop)
/// from worker code.
///
/// Worker-bound schedulers and [`Context`](crate::task::Context) do not keep
/// the runtime running after its owner is dropped.
///
/// # Examples
///
/// Submit work from synchronous code and stop after receiving its result:
///
/// ```
/// use arty::runtime::Runtime;
///
/// let runtime = Runtime::new()?;
/// let task = runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 });
/// assert_eq!(futures::executor::block_on(task)?, 42);
/// runtime.stop()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Runtime {
    scheduler: RuntimeScheduler,
    worker_joiner: WorkerJoiner,
    shutdown_on_drop: bool,
}

impl Runtime {
    /// Creates and starts a runtime with the default configuration.
    ///
    /// Equivalent to [`Runtime::builder().build()`](RuntimeBuilder::build).
    /// The default monitoring policy is
    /// [`MonitorPolicy::enabled`](crate::runtime::MonitorPolicy::enabled).
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the processor selection cannot be satisfied.
    ///
    /// # Panics
    ///
    /// Panics if worker creation or initialization fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new() -> Result<Self, Error> {
        RuntimeBuilder::new().build()
    }

    /// Returns a builder for configuring a runtime before starting its workers.
    ///
    /// See [`RuntimeBuilder`] for the defaults and available settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool(BlockingPoolPolicy::shared().max(4));
    /// ```
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Borrows the runtime's scheduler for runtime-wide submissions.
    ///
    /// Use it to submit work from outside the runtime and let the runtime
    /// choose a worker. In contrast,
    /// [`Context::scheduler`](crate::task::Context::scheduler) keeps child
    /// tasks on their parent's worker.
    ///
    /// The scheduler cannot be cloned or retained independently of this runtime.
    /// Selection does not imply execution or completion order.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let scheduler = runtime.scheduler();
    /// let first = scheduler.spawn_anywhere((), |_, ()| async { 20 });
    /// let second = scheduler.spawn_anywhere((), |_, ()| async { 22 });
    /// assert_eq!(
    ///     futures::executor::block_on(first)? + futures::executor::block_on(second)?,
    ///     42
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    #[inline]
    pub fn scheduler(&self) -> &RuntimeScheduler {
        &self.scheduler
    }

    /// Consumes this runtime, requests shutdown, and waits for its workers to stop.
    ///
    /// Cancels pending async tasks and prevents queued blocking callbacks from
    /// starting. Already-running blocking callbacks are allowed to finish.
    /// `Ok(())` means shutdown has completed.
    ///
    /// Use [`Context::control`](crate::task::Context::control) to obtain
    /// runtime control and request shutdown without consuming the owner or
    /// blocking the caller.
    ///
    /// # Errors
    ///
    /// Returns an error if a worker panicked or an I/O driver failed during
    /// initialization or cycle execution. It does not return I/O driver shutdown
    /// failures. It joins all workers before it reports a failure. Task failures
    /// must be observed through their own join handles.
    ///
    /// # Panics
    ///
    /// Panics if called from an Arty task or blocking worker. Use
    /// [`RuntimeControl::request_stop`](crate::runtime::RuntimeControl::request_stop)
    /// to request shutdown from worker code.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let error = runtime.scheduler().block_on(async |cx| {
    ///     cx.control().request_stop();
    ///     cx.scheduler()
    ///         .spawn(async |_| 42)
    ///         .await
    ///         .expect_err("submission follows shutdown")
    /// })?;
    /// runtime.stop()?;
    /// assert!(error.is_shutdown());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(mut self) -> Result<(), Error> {
        if crate::runtime::worker::WorkerMarker::is_current() {
            let cause = self.scheduler.dispatcher.stop(arty_executor::ShutdownCause::default());
            crate::runtime::worker::asynchronous::LocalWorker::begin_executor_shutdown(&self.scheduler.dispatcher, cause);
            self.shutdown_on_drop = false;
            panic_shutdown_from_worker("Runtime::stop");
        }
        _ = self.scheduler.dispatcher.stop(arty_executor::ShutdownCause::default());
        self.shutdown_on_drop = false;
        self.wait()
    }

    fn wait(&self) -> Result<(), Error> {
        let wait_error = self.worker_joiner.join().err();
        if let Some(error) = self.scheduler.dispatcher.shutdown().error() {
            return Err(Error::shutdown_failure(error));
        }
        wait_error.map_or(Ok(()), Err)
    }

    pub(crate) const fn with_dispatcher(dispatcher: DispatcherClient, worker_joiner: WorkerJoiner) -> Self {
        Self {
            scheduler: RuntimeScheduler::new(dispatcher),
            worker_joiner,
            shutdown_on_drop: true,
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.shutdown_on_drop {
            return;
        }

        let cause = self.scheduler.dispatcher.stop(arty_executor::ShutdownCause::default());
        if crate::runtime::worker::WorkerMarker::is_current() {
            crate::runtime::worker::asynchronous::LocalWorker::begin_executor_shutdown(&self.scheduler.dispatcher, cause);
            if std::thread::panicking() {
                return;
            }
            panic_shutdown_from_worker("Runtime drop");
        }
        let _ = self.wait();
    }
}

#[cold]
#[inline(never)]
#[expect(clippy::panic, reason = "runtime ownership on a worker is a programming error")]
fn panic_shutdown_from_worker(operation: &str) -> ! {
    panic!("{operation} cannot run on an Arty worker; use RuntimeControl::request_stop")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::future::poll_fn;
    use std::task::{Poll, Waker};
    use std::thread;

    use performables::sync::channel;
    use thread_aware::ThreadBuilder;

    use super::*;
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::worker::blocking::{PanicPolicy, Pool, Worker};
    use crate::task::JoinError;

    thread_local! {
        static SHUTDOWN_GUARD: RefCell<Option<ShutdownGuard>> = const { RefCell::new(None) };
    }

    struct ShutdownGuard(crate::runtime::RuntimeControl);

    impl Drop for ShutdownGuard {
        fn drop(&mut self) {
            self.0.request_stop();
        }
    }

    #[test]
    fn explicit_stop_reports_a_worker_panic_after_joining() {
        let worker = thread::spawn(|| panic!("worker shutdown failure"));
        let endpoint = WorkerEndpoint {
            command_tx: channel::unbounded().0,
            waker: Waker::noop().clone(),
            thread: ThreadBuilder::default().build(worker.thread().id()),
            blocking_worker: Worker::new(Pool::new(None), crate::runtime::shutdown::Shutdown::default(), PanicPolicy::Ignore),
        };
        let dispatcher = DispatcherClient::new(DispatcherCore::new(
            nonempty::NonEmpty::new(endpoint),
            crate::runtime::shutdown::Shutdown::default(),
        ));
        let runtime = Runtime::with_dispatcher(dispatcher, WorkerJoiner::new(vec![worker]));
        assert!(runtime.stop().unwrap_err().to_string().contains("panicked"));
    }

    #[test]
    fn authoritative_shutdown_failure_precedes_later_worker_panic() {
        let worker = thread::spawn(|| panic!("worker teardown failure"));
        let endpoint = WorkerEndpoint {
            command_tx: channel::unbounded().0,
            waker: Waker::noop().clone(),
            thread: ThreadBuilder::default().build(worker.thread().id()),
            blocking_worker: Worker::new(Pool::new(None), crate::runtime::shutdown::Shutdown::default(), PanicPolicy::Ignore),
        };
        let dispatcher = DispatcherClient::new(DispatcherCore::new(
            nonempty::NonEmpty::new(endpoint),
            crate::runtime::shutdown::Shutdown::default(),
        ));
        let runtime = Runtime::with_dispatcher(dispatcher.clone(), WorkerJoiner::new(vec![worker]));
        dispatcher.stop(Error::new(std::io::Error::other("first runtime failure")).into_shutdown_cause());

        assert_eq!(runtime.stop().unwrap_err().to_string(), "first runtime failure");
    }

    #[test]
    fn task_worker_stop_panics() {
        let runtime = single_worker_runtime();
        let joiner = runtime.worker_joiner.clone_for_test();
        let scheduler = worker_scheduler(&runtime);

        let error = scheduler.spawn(async move |_| runtime.stop()).join().unwrap_err();

        assert_programming_error(error, "Runtime::stop");
        joiner.join().unwrap();
    }

    #[test]
    fn ignored_task_worker_stop_skips_a_later_active_task() {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .panic(crate::runtime::PanicPolicy::ignore())
            .build()
            .unwrap();
        let joiner = runtime.worker_joiner.clone_for_test();
        let scheduler = worker_scheduler(&runtime);
        let (stopper_initialized, stopper_waker) = channel::unbounded();
        let (later_initialized, later_waker) = channel::unbounded();
        let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let invoked_by_task = std::sync::Arc::clone(&invoked);
        let stopper = scheduler.spawn(move |_| {
            let mut runtime = Some(runtime);
            let mut initialized = Some(stopper_initialized);
            poll_fn(move |cx| {
                if let Some(initialized) = initialized.take() {
                    initialized.send(cx.waker().clone()).unwrap();
                    return Poll::<Result<(), Error>>::Pending;
                }
                runtime.take().unwrap().stop()?;
                Poll::Ready(Ok(()))
            })
        });
        let later = scheduler.spawn(move |_| {
            let mut initialized = Some(later_initialized);
            poll_fn(move |cx| {
                if let Some(initialized) = initialized.take() {
                    initialized.send(cx.waker().clone()).unwrap();
                    return Poll::Pending;
                }
                invoked_by_task.store(true, std::sync::atomic::Ordering::Release);
                Poll::Ready(())
            })
        });
        let stopper_waker = stopper_waker.recv().unwrap();
        let later_waker = later_waker.recv().unwrap();

        scheduler
            .spawn(move |_| async move {
                stopper_waker.wake_by_ref();
                later_waker.wake_by_ref();
            })
            .join()
            .unwrap();

        assert_programming_error(stopper.join().unwrap_err(), "Runtime::stop");
        assert!(later.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(std::sync::atomic::Ordering::Acquire));
        joiner.join().unwrap();
    }

    #[test]
    fn ignored_task_worker_drop_skips_a_later_active_task() {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .panic(crate::runtime::PanicPolicy::ignore())
            .build()
            .unwrap();
        let joiner = runtime.worker_joiner.clone_for_test();
        let scheduler = worker_scheduler(&runtime);
        let (stopper_initialized, stopper_waker) = channel::unbounded();
        let (later_initialized, later_waker) = channel::unbounded();
        let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let invoked_by_task = std::sync::Arc::clone(&invoked);
        let stopper = scheduler.spawn(move |_| {
            let mut runtime = Some(runtime);
            let mut initialized = Some(stopper_initialized);
            poll_fn(move |cx| {
                if let Some(initialized) = initialized.take() {
                    initialized.send(cx.waker().clone()).unwrap();
                    return Poll::Pending;
                }
                drop(runtime.take());
                Poll::Ready(())
            })
        });
        let later = scheduler.spawn(move |_| {
            let mut initialized = Some(later_initialized);
            poll_fn(move |cx| {
                if let Some(initialized) = initialized.take() {
                    initialized.send(cx.waker().clone()).unwrap();
                    return Poll::Pending;
                }
                invoked_by_task.store(true, std::sync::atomic::Ordering::Release);
                Poll::Ready(())
            })
        });
        let stopper_waker = stopper_waker.recv().unwrap();
        let later_waker = later_waker.recv().unwrap();

        scheduler
            .spawn(move |_| async move {
                stopper_waker.wake_by_ref();
                later_waker.wake_by_ref();
            })
            .join()
            .unwrap();

        assert_programming_error(stopper.join().unwrap_err(), "Runtime drop");
        assert!(later.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(std::sync::atomic::Ordering::Acquire));
        joiner.join().unwrap();
    }

    #[test]
    fn blocking_worker_stop_panics() {
        let runtime = single_worker_runtime();
        let joiner = runtime.worker_joiner.clone_for_test();
        let scheduler = worker_scheduler(&runtime);

        let error = scheduler.spawn_blocking(move || runtime.stop()).join().unwrap_err();

        assert_programming_error(error, "Runtime::stop");
        joiner.join().unwrap();
    }

    #[test]
    fn task_worker_owner_drop_panics() {
        let runtime = single_worker_runtime();
        let joiner = runtime.worker_joiner.clone_for_test();
        let scheduler = worker_scheduler(&runtime);

        let error = scheduler.spawn(async move |_| drop(runtime)).join().unwrap_err();

        assert_programming_error(error, "Runtime drop");
        joiner.join().unwrap();
    }

    #[test]
    fn blocking_worker_owner_drop_panics() {
        let runtime = single_worker_runtime();
        let joiner = runtime.worker_joiner.clone_for_test();
        let scheduler = worker_scheduler(&runtime);

        let error = scheduler.spawn_blocking(move || drop(runtime)).join().unwrap_err();

        assert_programming_error(error, "Runtime drop");
        joiner.join().unwrap();
    }

    #[test]
    fn thread_local_shutdown_guard_survives_worker_slot_teardown() {
        thread::spawn(|| {
            let runtime = single_worker_runtime();
            let (control, scheduler) = runtime
                .scheduler()
                .block_on(async |cx| (cx.control().clone(), cx.scheduler().clone()))
                .unwrap();
            SHUTDOWN_GUARD.with(|guard| {
                assert!(guard.replace(Some(ShutdownGuard(control))).is_none());
            });
            scheduler.spawn(async |_| {}).join().unwrap();
            runtime.stop().unwrap();
        })
        .join()
        .unwrap();
    }

    fn assert_programming_error(error: JoinError, operation: &str) {
        assert!(error.is_panic());
        let panic = error.into_panic().expect("the join error was checked as a panic");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("runtime ownership panic uses a string payload");
        assert!(message.contains(operation));
        assert!(message.contains("RuntimeControl::request_stop"));
    }

    fn worker_scheduler(runtime: &Runtime) -> crate::task::Scheduler {
        runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
            .join()
            .unwrap()
    }

    fn single_worker_runtime() -> Runtime {
        Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .blocking_pool(crate::runtime::BlockingPoolPolicy::shared().max(1))
            .build()
            .unwrap()
    }
}
