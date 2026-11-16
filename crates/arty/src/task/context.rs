// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Access to a worker's scheduler, clock, and I/O.

use performables::arc::Arc;
use thread_aware::{Thread, ThreadAware};
use tick::Clock;

use crate::io::Io;
use crate::runtime::RuntimeControl;
use crate::runtime::dispatch::DispatcherClient;
use crate::task::scheduler::Scheduler;

/// Services supplied to an async task on its worker.
///
/// It is not `std::task::Context`, which Rust passes to a future's `poll` method.
///
/// Use [`scheduler`](Self::scheduler) for child tasks and [`clock`](Self::clock)
/// for delays and timeouts. Arty passes this value to the function that
/// creates a task's future.
///
/// New tasks receive their own `Context`. Cloning or moving an existing value
/// does not change its worker or keep its runtime running.
///
/// Relocation updates a value's state for a selected worker.
/// [`Scheduler::spawn_anywhere`] can relocate an existing `Context` to
/// another ready worker of the same runtime.
/// Relocation to a worker from another runtime or one that is not ready leaves
/// the context unchanged.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
/// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
/// let answer = cx
///     .scheduler()
///     .spawn(async |child| {
///         child
///             .clock()
///             .delay(std::time::Duration::from_millis(1))
///             .await;
///         42
///     })
///     .await?;
/// assert_eq!(answer, 42);
/// # Ok(())
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[derive(Debug, Clone)]
pub struct Context {
    inner: Arc<ContextInner>,
}

#[derive(Debug, Clone)]
struct ContextInner {
    scheduler: Scheduler,
    control: RuntimeControl,
    thread: Thread,
    clock: Clock,
    io: Io,
}

impl Context {
    /// Returns a scheduler that creates tasks on this context's worker.
    ///
    /// A clone still targets the same worker, even if used from another
    /// thread. Use [`Scheduler::spawn_anywhere`] to let the runtime choose
    /// a worker for new tasks instead.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
    /// let parent_thread = cx.thread().id();
    /// let child_thread = cx
    ///     .scheduler()
    ///     .spawn(async |child| child.thread().id())
    ///     .await?;
    /// assert_eq!(child_thread, parent_thread);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn scheduler(&self) -> &Scheduler {
        &self.inner.scheduler
    }

    /// Returns runtime-wide control available to this task.
    ///
    /// Use [`RuntimeControl::workers`] to list the runtime's workers.
    /// Use [`RuntimeControl::request_stop`] to request shutdown without waiting.
    #[must_use]
    #[inline]
    pub fn control(&self) -> &RuntimeControl {
        &self.inner.control
    }

    /// Returns this context's worker details, including its thread and NUMA node.
    ///
    /// A NUMA node groups processors near the same memory.
    ///
    /// Moving `Context` to another thread does not change these details unless
    /// you relocate it.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) {
    /// assert_eq!(cx.thread().id(), std::thread::current().id());
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn thread(&self) -> &Thread {
        &self.inner.thread
    }

    /// Returns this context's worker clock.
    ///
    /// Use it for time queries, delays, stopwatches, and timeouts. The runtime
    /// wakes tasks when their timers expire while the worker runs.
    /// With controlled time, use `ClockControl` to advance time.
    /// Keeping the clock does not keep that worker alive.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) {
    /// let duration = std::time::Duration::from_millis(1);
    /// let watch = cx.clock().stopwatch();
    /// cx.clock().delay(duration).await;
    /// assert!(watch.elapsed() >= duration);
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn clock(&self) -> &Clock {
        &self.inner.clock
    }

    /// Returns access to this runtime's I/O contexts.
    #[must_use]
    #[inline]
    pub fn io(&self) -> &Io {
        &self.inner.io
    }
}

impl ThreadAware for Context {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.inner.thread == *destination {
            return;
        }

        // The scheduler validates runtime ownership before resolving a worker.
        let Some(worker_index) = self.inner.scheduler.resolve_worker_index(destination) else {
            return;
        };
        let inner = Arc::make_mut(&mut self.inner);
        let source = inner.thread.clone();

        inner.scheduler.relocate_to_worker(destination, worker_index);
        inner.thread = destination.clone();
        inner.clock.relocate(Some(&source), destination);
        inner.io.relocate(Some(&source), destination);
    }
}

impl Context {
    pub(crate) fn new(dispatcher: DispatcherClient, clock: Clock, io: Io, thread: Thread) -> Self {
        Self {
            inner: Arc::new(ContextInner {
                scheduler: Scheduler::new(dispatcher.clone(), thread.clone()),
                control: RuntimeControl::new(dispatcher),
                thread,
                clock,
                io,
            }),
        }
    }

    pub(crate) fn dispatcher(&self) -> &DispatcherClient {
        &self.inner.scheduler.dispatcher
    }
}

impl AsRef<Scheduler> for Context {
    #[inline]
    fn as_ref(&self) -> &Scheduler {
        self.scheduler()
    }
}

impl AsRef<Clock> for Context {
    #[inline]
    fn as_ref(&self) -> &Clock {
        self.clock()
    }
}
