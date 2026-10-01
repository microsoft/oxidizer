// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::builder::RuntimeBuilder;
use crate::runtime::context::SharedState;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::error::Error;
use crate::runtime::thread::assert_not_flagged;
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
/// # Shutdown
///
/// Dropping the owner requests shutdown and waits for workers to stop. Pending
/// asynchronous tasks and queued blocking callbacks are cancelled; blocking
/// callbacks already running are allowed to finish. A blocking callback that
/// never returns can therefore prevent shutdown from completing.
///
/// Dropping the owner from one of its own blocking callbacks requests shutdown
/// without waiting for that callback to finish. Worker-bound schedulers and
/// [`Builtins`](crate::task::Builtins) do not keep the runtime running after its owner is dropped.
///
/// # Panics
///
/// Dropping the owner on an asynchronous Arty worker panics. Keep ownership on
/// a thread where blocking is allowed.
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
/// runtime.stop();
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Runtime {
    scheduler: RuntimeScheduler,
    pub(in crate::runtime) shared_state: SharedState,
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
    /// finish. Calling this from one of the runtime's own blocking callbacks
    /// requests shutdown without waiting for that callback.
    ///
    /// Use [`RuntimeOperations::request_stop`](crate::runtime::RuntimeOperations::request_stop)
    /// to request shutdown without consuming the owner or blocking the caller.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous Arty worker.
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
    /// runtime.stop();
    /// let error = scheduler
    ///     .spawn(async |_| 42)
    ///     .wait()
    ///     .expect_err("submission follows shutdown");
    /// assert!(error.is_shutdown());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(self) {
        drop(self);
    }

    fn wait(&self) {
        assert_not_flagged();
        assert!(
            !self.scheduler.dispatcher.is_current_blocking_task(),
            "a runtime blocking task cannot wait for its own shutdown"
        );
        self.scheduler.dispatcher.wait();
    }

    pub(in crate::runtime) const fn with_dispatcher(dispatcher: DispatcherClient, shared_state: SharedState) -> Self {
        Self {
            scheduler: RuntimeScheduler::new(dispatcher),
            shared_state,
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.scheduler.dispatcher.stop();
        if !self.scheduler.dispatcher.is_current_blocking_task() {
            self.wait();
        }
    }
}
