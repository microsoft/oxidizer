// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::Workers;
use crate::runtime::dispatch::DispatcherClient;
use crate::task::Scheduler;

/// A handle for requesting runtime shutdown.
///
/// Borrow this handle through [`Builtins::control`](crate::task::Builtins::control).
/// It can be cloned and used from any thread without keeping the runtime alive
/// or retaining an association with the task's worker.
///
/// [`workers`](Self::workers) exposes the runtime's worker coordinates, and
/// [`request_stop`](Self::request_stop) initiates shutdown without waiting.
#[derive(Debug, Clone)]
pub struct RuntimeControl {
    dispatcher: DispatcherClient,
}

impl RuntimeControl {
    pub(crate) const fn new(dispatcher: DispatcherClient) -> Self {
        Self { dispatcher }
    }

    /// Returns this runtime's worker coordinates.
    ///
    /// The coordinates remain owned by the runtime. Use them with
    /// [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at) to target a
    /// specific worker.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    /// let tasks: Vec<_> = cx
    ///     .control()
    ///     .workers()
    ///     .into_iter()
    ///     .map(|worker| {
    ///         cx.scheduler()
    ///             .spawn_at(worker, (), |()| async { std::thread::current().id() })
    ///     })
    ///     .collect();
    /// for task in tasks {
    ///     let _worker_id = task.await?;
    /// }
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    pub fn workers(&self) -> &Workers {
        self.dispatcher.workers()
    }

    /// Registers a callback for the end of runtime shutdown.
    ///
    /// The runtime runs the callback once after all async tasks are cancelled,
    /// all blocking callbacks finish or are rejected, and worker driver
    /// teardown finishes. Runtime shutdown waits for the callback to return.
    ///
    /// The runtime is already in shutdown mode when the callback runs.
    /// A captured [`Scheduler`] rejects new async tasks and blocking callbacks
    /// with the normal shutdown [`JoinError`](crate::task::JoinError). It does
    /// not run the submitted body.
    ///
    /// Callbacks registered while shutdown callbacks are running join the same
    /// completion barrier.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`](crate::runtime::Error) if shutdown callbacks already
    /// completed. The callback does not run in that case.
    #[doc(hidden)]
    pub fn on_shutdown<F>(&self, callback: F) -> Result<(), crate::runtime::Error>
    where
        F: FnOnce() + Send + 'static,
    {
        self.dispatcher.core.on_shutdown(callback)
    }

    /// Requests shutdown without blocking the calling thread.
    ///
    /// May be called repeatedly from any thread. Pending tasks are cancelled,
    /// and new submissions receive [`JoinError`](crate::task::JoinError).
    /// Running blocking callbacks are allowed to finish. The owner remains
    /// responsible for waiting for shutdown when it is stopped or dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let control = runtime
    ///     .scheduler()
    ///     .block_on(async |cx| cx.control().clone())?;
    /// control.request_stop();
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg_attr(test, mutants::skip)] // It is impractical to test for "stuff not happening", so mutating this easily leads to timeouts.
    pub fn request_stop(&self) {
        let cause = self.dispatcher.stop();
        Scheduler::begin_current_executor_shutdown(&self.dispatcher, cause);
    }
}
