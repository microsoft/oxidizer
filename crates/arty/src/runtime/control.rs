// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::Workers;
use crate::runtime::dispatch::DispatcherClient;

/// A handle for requesting runtime shutdown.
///
/// Borrow this handle through [`Context::control`](crate::task::Context::control).
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
    /// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
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

    /// Requests shutdown without blocking the calling thread.
    ///
    /// You can call this method repeatedly from any thread. Pending tasks are cancelled,
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
        let cause = self.dispatcher.stop(arty_executor::ShutdownCause::default());
        crate::runtime::worker::asynchronous::LocalWorker::begin_executor_shutdown(&self.dispatcher, cause);
    }
}
