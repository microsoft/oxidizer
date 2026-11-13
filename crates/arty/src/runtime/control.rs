// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::Workers;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::hook::{Hook, HookFn, registration};

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

    /// Registers a runtime hook on every live async worker.
    ///
    /// Arty clones the hook data for each worker and relocates it before the
    /// callback can run. On a runtime worker, Arty applies the hook to that
    /// worker before this method returns. Arty sends registration to other
    /// workers through their normal command queues. A worker applies the hook
    /// when it processes that command. If runtime shutdown has started, Arty
    /// ignores the registration.
    ///
    /// Arty does not catch a panic from the callback. You must make sure that
    /// the callback completes without a panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    /// use arty::runtime::hook::Panic;
    ///
    /// let runtime = Runtime::new()?;
    /// let control = runtime
    ///     .scheduler()
    ///     .block_on(async |cx| cx.control().clone())?;
    /// control.on(Panic::IGNORE);
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn on<H, D>(&self, hook: impl Into<HookFn<H, D>>)
    where
        H: Hook,
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        let hook = registration(hook.into());
        self.dispatcher.register_hook(&hook);
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
    pub fn request_stop(&self) {
        let cause = self.dispatcher.stop(arty_executor::ShutdownCause::default());
        crate::runtime::worker::asynchronous::LocalWorker::begin_executor_shutdown(&self.dispatcher, cause);
    }
}
