// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use crate::{BoxedBlockingTask, BoxedFuture, SpawnCustom, Spawner, ThreadAwareAsyncFnOnce};

impl Spawner {
    /// Creates a spawner bound to an Arty worker scheduler.
    ///
    /// Tasks retain the scheduler's worker association. Cloning or relocating
    /// the `Spawner` follows [`Scheduler`](::arty::task::Scheduler) semantics,
    /// and the scheduler does not keep its runtime running.
    ///
    /// # Panics
    ///
    /// Awaiting a returned [`JoinHandle`](crate::JoinHandle) will panic if Arty
    /// reports a task panic, cancellation, or shutdown rejection.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # fn main() {
    /// # #[cfg(feature = "arty")] {
    /// # (|| {
    /// use anyspawn::Spawner;
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let result = runtime.scheduler().block_on(async |cx| {
    ///     let spawner = Spawner::new_arty(cx.scheduler().clone());
    ///     spawner.spawn(async { 42 }).await
    /// })?;
    /// assert_eq!(result, 42);
    /// runtime.stop()?;
    /// # Ok::<(), arty::runtime::Error>(())
    /// # })().unwrap();
    /// # }
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "arty")))]
    pub fn new_arty(scheduler: ::arty::task::Scheduler) -> Self {
        Self::new_custom("arty", ArtySpawner(scheduler))
    }
}

#[derive(Clone, ThreadAware)]
struct ArtySpawner(::arty::task::Scheduler);

impl SpawnCustom for ArtySpawner {
    fn spawn(&self, task: BoxedFuture) {
        drop(self.0.spawn(move |_| task));
    }

    fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        drop(self.0.spawn_anywhere(task, |task| task.call_once()));
    }

    fn spawn_blocking(&self, task: BoxedBlockingTask) {
        drop(self.0.spawn_blocking(task));
    }
}
