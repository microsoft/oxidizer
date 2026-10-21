// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::slice;

use thread_aware::Thread;

use crate::runtime::dispatch::DispatcherClient;
use crate::task::Scheduler;

/// A handle for requesting runtime shutdown.
///
/// Borrow this handle through [`Builtins::operations`](crate::task::Builtins::operations).
/// It can be cloned and used from any thread without keeping the runtime alive
/// or retaining an association with the task's worker.
///
/// [`workers`](Self::workers) exposes the runtime's worker coordinates, and
/// [`request_stop`](Self::request_stop) initiates shutdown without waiting.
#[derive(Debug, Clone)]
pub struct RuntimeOperations {
    dispatcher: DispatcherClient,
}

impl RuntimeOperations {
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
    ///     .operations()
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
    /// let operations = runtime
    ///     .scheduler()
    ///     .block_on(async |cx| cx.operations().clone())?;
    /// operations.request_stop();
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg_attr(test, mutants::skip)] // It is impractical to test for "stuff not happening", so mutating this easily leads to timeouts.
    pub fn request_stop(&self) {
        Scheduler::begin_current_executor_shutdown(&self.dispatcher);
        self.dispatcher.stop();
    }
}

/// A runtime's worker coordinates.
///
/// Obtain this view from [`RuntimeOperations::workers`]. Each item is borrowed
/// from the runtime and can be passed directly to
/// [`Scheduler::spawn_at`](crate::task::Scheduler::spawn_at).
#[derive(Debug)]
pub struct Workers {
    inner: Box<[Thread]>,
}

impl Workers {
    pub(crate) fn new(inner: Box<[Thread]>) -> Self {
        Self { inner }
    }

    /// Iterates over the runtime's worker coordinates.
    pub fn iter(&self) -> slice::Iter<'_, Thread> {
        self.inner.iter()
    }

    /// Returns the number of workers in the runtime.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the runtime has no workers.
    ///
    /// A running Arty runtime always has at least one worker.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<'a> IntoIterator for &'a Workers {
    type Item = &'a Thread;
    type IntoIter = slice::Iter<'a, Thread>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
