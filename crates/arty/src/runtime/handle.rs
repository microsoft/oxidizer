// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::builder::RuntimeBuilder;
use crate::runtime::context::SharedState;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::error::Error;
use crate::runtime::thread::is_flagged;
use crate::task::RuntimeScheduler;

/// Owns an Arty runtime's workers and their shutdown.
///
/// Use this type to run asynchronous work from synchronous code. Construction
/// starts one asynchronous worker per selected processor. A task stays on its
/// worker until it finishes or is cancelled.
///
/// Borrow its [`RuntimeScheduler`] through
/// [`scheduler`](Self::scheduler) to submit work or block on
/// tasks that borrow caller-owned data. Consume the owner with [`stop`](Self::stop)
/// when the required work has finished.
///
/// # Drop
///
/// Dropping the owner requests shutdown and normally waits for workers to stop. Pending
/// asynchronous tasks and queued blocking callbacks are cancelled; blocking
/// callbacks already running are allowed to finish. A blocking callback that
/// never returns can therefore prevent shutdown from completing.
///
/// Implicit cleanup cannot return shutdown errors. Worker panic diagnostics are
/// still emitted; use [`stop`](Self::stop) to receive the shutdown outcome.
///
/// On any asynchronous Arty worker or one of this runtime's blocking callbacks,
/// dropping the owner only requests shutdown and returns without waiting.
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
/// let task = runtime.scheduler().spawn_anywhere(async |_| 42);
/// assert_eq!(task.wait()?, 42);
/// runtime.stop()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Runtime {
    scheduler: RuntimeScheduler,
    pub(in crate::runtime) shared_state: SharedState,
    shutdown_on_drop: bool,
}

impl Runtime {
    /// Creates and starts a runtime with the default configuration.
    ///
    /// Equivalent to [`Runtime::builder().build()`](RuntimeBuilder::build).
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
    /// let builder = Runtime::builder().blocking_pool_policy(BlockingPoolPolicy::shared(4));
    /// ```
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Borrows the runtime's scheduler for runtime-wide submissions.
    ///
    /// Use it to submit work from outside the runtime or distribute independent
    /// tasks across workers. In contrast, [`Builtins::scheduler`](crate::task::Builtins::scheduler) keeps child
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
    /// let first = scheduler.spawn_anywhere(async |_| 20);
    /// let second = scheduler.spawn_anywhere(async |_| 22);
    /// assert_eq!(first.wait()? + second.wait()?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    #[inline]
    pub fn scheduler(&self) -> &RuntimeScheduler {
        &self.scheduler
    }

    /// Consumes this runtime, requests shutdown, and waits for its workers to stop.
    ///
    /// Cancels pending asynchronous and local tasks and prevents queued blocking
    /// callbacks from starting. Already-running blocking callbacks are allowed to
    /// finish. Shutdown is still requested when the calling context cannot wait.
    /// `Ok(())` means shutdown has completed. A calling-context error does not
    /// mean the workers have stopped.
    ///
    /// Use [`RuntimeOperations::request_stop`](crate::runtime::RuntimeOperations::request_stop)
    /// to request shutdown without consuming the owner or blocking the caller.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if called from an asynchronous Arty worker or one of
    /// this runtime's blocking callbacks, because those contexts cannot wait for
    /// shutdown. In those cases the owner is consumed without waiting.
    ///
    /// Also returns an error if a worker panicked. All workers are joined before
    /// reporting a worker failure; task failures must be observed through their
    /// own join handles.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let scheduler = runtime
    ///     .scheduler()
    ///     .spawn_anywhere(async |cx| cx.scheduler().clone())
    ///     .wait()?;
    /// runtime.stop()?;
    /// let error = scheduler
    ///     .spawn(async |_| 42)
    ///     .wait()
    ///     .expect_err("submission follows shutdown");
    /// assert!(error.is_shutdown());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(mut self) -> Result<(), Error> {
        self.scheduler.dispatcher.stop();
        self.shutdown_on_drop = false;
        self.wait()
    }

    fn wait(&self) -> Result<(), Error> {
        if is_flagged() {
            return Err(Error::new("an asynchronous Arty worker cannot wait for runtime shutdown"));
        }
        if self.scheduler.dispatcher.is_current_blocking_task() {
            return Err(Error::new("a runtime blocking callback cannot wait for its own shutdown"));
        }
        self.scheduler.dispatcher.wait()
    }

    pub(in crate::runtime) const fn with_dispatcher(dispatcher: DispatcherClient, shared_state: SharedState) -> Self {
        Self {
            scheduler: RuntimeScheduler::new(dispatcher),
            shared_state,
            shutdown_on_drop: true,
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.shutdown_on_drop {
            return;
        }

        self.scheduler.dispatcher.stop();
        if !is_flagged() && !self.scheduler.dispatcher.is_current_blocking_task() {
            // Worker entry wrappers report panic diagnostics; only explicit stop can return errors.
            let _ = self.wait();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::{Arc, mpsc};
    use std::task::Waker;
    use std::thread;

    use observed::Sink;
    use testing_aids::TEST_TIMEOUT;
    use thread_aware::ThreadBuilder;

    use super::*;
    use crate::runtime::blocking_worker::{BlockingPool, BlockingWorker};
    use crate::runtime::dispatch::{DispatcherCore, WorkerEndpoint};
    use crate::runtime::thread::waiter::ThreadWaiter;

    #[test]
    fn explicit_stop_reports_a_worker_panic_after_joining() {
        let worker = thread::spawn(|| panic!("worker shutdown failure"));
        let endpoint = WorkerEndpoint {
            command_tx: mpsc::channel().0,
            waker: Waker::noop().clone(),
            thread: ThreadBuilder::default().build(worker.thread().id()),
            blocking_worker: BlockingWorker::new(BlockingPool::new(None), Sink::noop()),
        };
        let dispatcher = DispatcherClient::new(Arc::new(DispatcherCore::new(
            ThreadWaiter::new(vec![worker]),
            nonempty::NonEmpty::new(endpoint),
            Sink::noop(),
        )));
        let runtime = Runtime::with_dispatcher(dispatcher, vec![].into());
        assert!(runtime.stop().unwrap_err().to_string().contains("panicked"));
    }

    #[test]
    fn explicit_stop_on_an_async_worker_returns_an_error_without_unwinding() {
        let runtime = Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(1))
            .build()
            .unwrap();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere(async |cx| cx.scheduler().clone())
            .wait()
            .unwrap();
        // Check admission before moving the owner into code that would self-join if the guard failed.
        assert!(scheduler.spawn(async |_| is_flagged()).wait().unwrap());
        let outcome = scheduler.spawn(async move |_| runtime.stop()).wait().unwrap();
        assert!(outcome.unwrap_err().to_string().contains("asynchronous Arty worker"));
    }

    #[test]
    fn explicit_stop_in_its_blocking_callback_returns_an_error_without_self_joining() {
        let runtime = Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(1))
            .build()
            .unwrap();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere(async |cx| cx.scheduler().clone())
            .wait()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();
        assert!(
            scheduler
                .spawn_blocking(move || dispatcher.is_current_blocking_task())
                .wait()
                .unwrap()
        );
        let outcome = scheduler.spawn_blocking(move || runtime.stop()).wait().unwrap();
        assert!(outcome.unwrap_err().to_string().contains("blocking callback"));
    }

    #[test]
    fn dropping_the_owner_on_its_async_worker_requests_shutdown_without_unwinding() {
        let runtime = Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(1))
            .build()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere(async |cx| cx.scheduler().clone())
            .wait()
            .unwrap();
        assert!(scheduler.spawn(async |_| is_flagged()).wait().unwrap());

        scheduler.spawn(async move |_| drop(runtime)).wait().unwrap();

        assert!(dispatcher.is_shutting_down());
        dispatcher.wait().unwrap();
        assert!(scheduler.spawn(async |_| 42).wait().unwrap_err().is_shutdown());
    }

    #[test]
    fn dropping_another_runtime_on_a_worker_does_not_wait_for_blocking_work() {
        let runtime = Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(1))
            .build()
            .unwrap();
        let caller = Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(1))
            .build()
            .unwrap();
        let dispatcher = runtime.scheduler.dispatcher.clone();
        let (started, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let blocking = runtime.scheduler().spawn_blocking(move || {
            started.send(()).unwrap();
            released.recv_timeout(TEST_TIMEOUT).unwrap();
            42
        });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        assert!(caller.scheduler().spawn_anywhere(async |_| is_flagged()).wait().unwrap());

        caller.scheduler().spawn_anywhere(async move |_| drop(runtime)).wait().unwrap();

        assert!(dispatcher.is_shutting_down());
        release.send(()).unwrap();
        assert_eq!(blocking.wait().unwrap(), 42);
        dispatcher.wait().unwrap();
        caller.stop().unwrap();
    }
}
