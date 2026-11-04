// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::Workers;
use crate::runtime::dispatch::DispatcherClient;

/// Requests runtime shutdown from any thread.
///
/// Obtain runtime control through [`Context::control`](crate::task::Context::control).
/// You can clone it and use it from any thread.
/// It does not keep the runtime alive and is not tied to the task's worker.
///
/// [`workers`](Self::workers) lists the runtime's workers.
/// [`request_stop`](Self::request_stop) starts shutdown without waiting.
#[derive(Debug, Clone)]
pub struct RuntimeControl {
    dispatcher: DispatcherClient,
}

impl RuntimeControl {
    pub(crate) const fn new(dispatcher: DispatcherClient) -> Self {
        Self { dispatcher }
    }

    /// Returns the details of this runtime's workers.
    ///
    /// The runtime owns these details. Use them with
    /// [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at) to select a
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
    /// You can call this method repeatedly from any thread.
    /// Arty cancels pending async tasks and discards queued blocking functions.
    /// New tasks receive [`JoinError`](crate::task::JoinError).
    /// Blocking functions that already started can finish.
    /// Call [`Runtime::stop`](crate::runtime::Runtime::stop) or drop the runtime
    /// to wait for shutdown to finish.
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
