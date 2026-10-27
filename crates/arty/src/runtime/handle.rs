// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::arc::Arc;

use crate::runtime::blocking_worker;
use crate::runtime::builder::RuntimeBuilder;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::error::Error;
use crate::task::{RuntimeScheduler, Scheduler};

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
/// Implicit cleanup cannot return shutdown errors. Worker panic and I/O driver
/// diagnostics are still emitted; use [`stop`](Self::stop) to receive the
/// shutdown outcome.
///
/// On any asynchronous Arty worker or one of this runtime's blocking worker
/// threads, including callback teardown, dropping the owner only requests
/// shutdown and returns without waiting.
/// The workers complete their cleanup independently; destruction in those
/// contexts is not a shutdown-completion barrier.
///
/// Worker-bound schedulers and [`Builtins`](crate::task::Builtins) do not keep
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
    /// [`Builtins::scheduler`](crate::task::Builtins::scheduler) keeps child
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
    /// Shutdown is still requested when the calling context cannot wait.
    /// `Ok(())` means shutdown has completed. A calling-context error does not
    /// mean the workers have stopped.
    ///
    /// Use [`Builtins::control`](crate::task::Builtins::control) to obtain
    /// runtime control and request shutdown without consuming the owner or
    /// blocking the caller.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if called from an async Arty worker or one of this
    /// runtime's blocking worker threads, including callback teardown, because
    /// those contexts cannot wait for shutdown. In those cases the owner is
    /// consumed without waiting.
    ///
    /// Also returns an error if a worker panicked or an I/O driver failed during
    /// initialization or cycle execution. Driver shutdown errors are reported
    /// through telemetry while shutdown continues. All workers are joined
    /// before reporting a failure; task failures must be observed through their
    /// own join handles.
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
        _ = self.scheduler.dispatcher.stop();
        self.shutdown_on_drop = false;
        self.wait()
    }

    fn wait(&self) -> Result<(), Error> {
        let current_thread = std::thread::current().id();
        let is_registered_worker = self.scheduler.dispatcher.worker_index(current_thread).is_some();
        if Scheduler::is_current_worker_thread() || is_registered_worker {
            crate::runtime::dispatch::DispatcherCore::wait_for_shutdown_in_background(Arc::clone(&self.scheduler.dispatcher.core));
            return Err(Error::shutdown_wait_from_worker());
        }
        if blocking_worker::is_current_blocking_thread() || self.scheduler.dispatcher.is_current_blocking_thread() {
            crate::runtime::dispatch::DispatcherCore::wait_for_shutdown_in_background(Arc::clone(&self.scheduler.dispatcher.core));
            return Err(Error::shutdown_wait_from_blocking_callback());
        }
        let wait_error = self.scheduler.dispatcher.wait().err();
        self.scheduler.dispatcher.core.finish_shutdown_after_wait();
        if let Some(error) = self.scheduler.dispatcher.shutdown().error() {
            return Err(Error::shutdown_failure(error));
        }
        wait_error.map_or(Ok(()), Err)
    }

    pub(crate) const fn with_dispatcher(dispatcher: DispatcherClient) -> Self {
        Self {
            scheduler: RuntimeScheduler::new(dispatcher),
            shutdown_on_drop: true,
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.shutdown_on_drop {
            return;
        }

        _ = self.scheduler.dispatcher.stop();
        // `wait` rejects self-waits from async workers and blocking callbacks;
        // worker entry wrappers report any resulting failure.
        let _ = self.wait();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod worker_wait_guards_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::mpsc;
    use std::task::Waker;
    use std::thread;

    use observed::Sink;
    use performables::sync::channel::{self, oneshot};
    use testing_aids::TEST_TIMEOUT;
    use thread_aware::{ThreadBuilder, Unaware};

    use super::*;
    use crate::runtime::blocking_worker::{BlockingPool, BlockingWorker};
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::thread::waiter::ThreadWaiter;

    #[test]
    fn explicit_stop_reports_a_worker_panic_after_joining() {
        let worker = thread::spawn(|| panic!("worker shutdown failure"));
        let endpoint = WorkerEndpoint {
            command_tx: channel::unbounded().0,
            waker: Waker::noop().clone(),
            thread: ThreadBuilder::default().build(worker.thread().id()),
            blocking_worker: BlockingWorker::new(BlockingPool::new(None), Sink::noop(), crate::runtime::shutdown::Shutdown::default()),
        };
        let dispatcher = DispatcherClient::new(Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![worker]),
            nonempty::NonEmpty::new(endpoint),
            Sink::noop(),
            crate::runtime::shutdown::Shutdown::default(),
        )));
        let runtime = Runtime::with_dispatcher(dispatcher);
        assert!(runtime.stop().unwrap_err().to_string().contains("panicked"));
    }

    #[test]
    fn authoritative_shutdown_failure_precedes_later_worker_panic() {
        let worker = thread::spawn(|| panic!("worker teardown failure"));
        let endpoint = WorkerEndpoint {
            command_tx: channel::unbounded().0,
            waker: Waker::noop().clone(),
            thread: ThreadBuilder::default().build(worker.thread().id()),
            blocking_worker: BlockingWorker::new(BlockingPool::new(None), Sink::noop(), crate::runtime::shutdown::Shutdown::default()),
        };
        let dispatcher = DispatcherClient::new(Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![worker]),
            nonempty::NonEmpty::new(endpoint),
            Sink::noop(),
            crate::runtime::shutdown::Shutdown::default(),
        )));
        let runtime = Runtime::with_dispatcher(dispatcher.clone());
        dispatcher.stop_with_error(Error::new(std::io::Error::other("first runtime failure")));

        assert_eq!(runtime.stop().unwrap_err().to_string(), "first runtime failure");
    }

    #[test]
    fn blocking_callback_is_detected_before_waiting_for_shutdown() {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();

        assert!(
            runtime
                .scheduler()
                .spawn_blocking(move || dispatcher.is_current_blocking_thread())
                .join()
                .unwrap()
        );

        runtime.stop().unwrap();
    }

    #[test]
    fn dropping_the_owner_on_its_async_worker_requests_shutdown_without_unwinding() {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
            .join()
            .unwrap();
        assert!(scheduler.spawn(async |_| Scheduler::is_current_worker_thread()).join().unwrap());

        scheduler.spawn(async move |_| drop(runtime)).join().unwrap();

        assert_eq!(dispatcher.shutdown().state(), crate::runtime::ShutdownState::Started);
        dispatcher.wait().unwrap();
        assert!(scheduler.spawn(async |_| 42).join().unwrap_err().is_shutdown());
    }

    #[test]
    fn dropping_the_owner_on_an_async_worker_does_not_wait_for_shutdown() {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
            .join()
            .unwrap();
        scheduler.spawn(async move |_| drop(runtime)).join().unwrap();
        assert_eq!(dispatcher.shutdown().state(), crate::runtime::ShutdownState::Started);
        dispatcher.wait().unwrap();
    }

    #[test]
    fn dropping_another_runtime_on_a_worker_does_not_wait_for_blocking_work() {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let caller = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();
        let (started, ready) = channel::unbounded();
        let (release, released) = channel::unbounded();
        let blocking = runtime.scheduler().spawn_blocking(move || {
            started.send(()).unwrap();
            released.recv_timeout(TEST_TIMEOUT).unwrap();
            42
        });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        assert!(
            caller
                .scheduler()
                .spawn_anywhere((), |_, ()| async { Scheduler::is_current_worker_thread() })
                .join()
                .unwrap()
        );

        caller
            .scheduler()
            .spawn_anywhere(Unaware(runtime), |_, Unaware(runtime)| async move { drop(runtime) })
            .join()
            .unwrap();

        assert_eq!(dispatcher.shutdown().state(), crate::runtime::ShutdownState::Started);
        release.send(()).unwrap();
        assert_eq!(blocking.join().unwrap(), 42);
        dispatcher.wait().unwrap();
        caller.stop().unwrap();
    }

    #[test]
    fn dropping_a_completed_runtime_on_a_foreign_worker_defers_callbacks() {
        let target = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let dispatcher = target.scheduler.dispatcher.clone();
        let control = target.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
        let (callback_started, callback_ready) = oneshot();
        let (release_callback, callback_gate) = mpsc::sync_channel(1);
        control.on_shutdown(move || {
            callback_started.send(()).unwrap();
            callback_gate.recv().unwrap();
        });
        control.request_stop();
        dispatcher.wait().unwrap();

        let caller = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let (dropped, drop_observed) = mpsc::sync_channel(1);
        let drop_target = caller
            .scheduler()
            .spawn_anywhere(Unaware((target, dropped)), |_, Unaware((target, dropped))| async move {
                drop(target);
                dropped.send(()).unwrap();
            });
        let (released, release_observed) = mpsc::sync_channel(1);
        let release = caller.scheduler().spawn_anywhere(
            Unaware((callback_ready, release_callback, released)),
            |_, Unaware((callback_ready, release_callback, released))| async move {
                callback_ready.await.unwrap();
                release_callback.send(()).unwrap();
                released.send(()).unwrap();
            },
        );

        drop_observed.recv_timeout(TEST_TIMEOUT).unwrap();
        release_observed.recv_timeout(TEST_TIMEOUT).unwrap();
        futures::executor::block_on(drop_target).unwrap();
        futures::executor::block_on(release).unwrap();
        caller.stop().unwrap();
    }
}
