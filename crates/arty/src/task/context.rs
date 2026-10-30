// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker services and thread-aware relocation.

use observed::Sink;
#[cfg(debug_assertions)]
use observed::emit;
use performables::arc::Arc;
use thread_aware::{Thread, ThreadAware};
use tick::Clock;

use crate::io::Io;
use crate::runtime::RuntimeControl;
use crate::runtime::dispatch::DispatcherClient;
#[cfg(debug_assertions)]
use crate::runtime::telemetry::events::{BacktraceText, ContextThreadMismatch, ThreadName};
use crate::task::scheduler::Scheduler;

/// Services supplied to an async task on its worker.
///
/// This is Arty's task-execution context, not `std::task::Context` or a future's
/// poll context.
///
/// Use [`scheduler`](Self::scheduler) for child tasks and [`clock`](Self::clock)
/// for delays and timeouts. Runtime task factories receive this value by
/// ownership.
///
/// New tasks receive their own `Context`. Cloning or moving an existing value
/// does not change its worker or keep its runtime running.
///
/// [`Scheduler::spawn_anywhere`] can relocate an existing `Context` to
/// another ready worker of the same runtime. A worker from another runtime, or
/// one that is not ready, leaves its association unchanged.
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
    sink: Sink,
}

impl Context {
    /// Returns a scheduler that creates tasks on the associated worker.
    ///
    /// A clone still targets the same worker, even if used from another
    /// thread. Use [`Scheduler::spawn_anywhere`] to let the runtime choose
    /// a worker for new work instead.
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
        self.validate();

        &self.inner.scheduler
    }

    /// Returns runtime-wide control available to this task.
    ///
    /// Use [`RuntimeControl::workers`] to inspect the runtime's worker
    /// coordinates and [`RuntimeControl::request_stop`] to request shutdown
    /// without waiting.
    #[must_use]
    #[inline]
    pub fn control(&self) -> &RuntimeControl {
        self.validate();

        &self.inner.control
    }

    /// Returns the associated worker's coordinate, including its NUMA locality.
    ///
    /// This describes where the services belong, not necessarily the thread
    /// executing the caller. Moving `Context` without relocation leaves this
    /// coordinate unchanged.
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
        self.validate();

        &self.inner.thread
    }

    /// Returns the associated worker's clock.
    ///
    /// Use it for time queries, delays, stopwatches, and timeouts. The runtime
    /// drives its timers while the worker is running; retaining the clock does
    /// not keep that worker alive.
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
        self.validate();

        &self.inner.clock
    }

    /// Returns access to I/O contexts hosted by this runtime.
    #[must_use]
    #[inline]
    pub fn io(&self) -> &Io {
        self.validate();
        &self.inner.io
    }

    /// Returns this runtime's telemetry sink.
    ///
    /// Emit application events through this [`Sink`] to use the same telemetry
    /// configuration as the runtime. It is a no-op sink unless one was provided
    /// with [`RuntimeBuilder::sink`](crate::runtime::RuntimeBuilder::sink).
    ///
    /// # Examples
    ///
    /// An application that also depends on `observed` can emit its own event:
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// #[observed::event("app.task.started")]
    /// #[info("task started")]
    /// struct TaskStarted;
    ///
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) {
    /// observed::emit!(cx.sink(), TaskStarted);
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn sink(&self) -> &Sink {
        self.validate();

        &self.inner.sink
    }

    #[cfg_attr(test, mutants::skip)]
    #[cfg(debug_assertions)]
    fn validate(&self) {
        let thread = std::thread::current();
        if thread.id() != self.inner.thread.id() {
            let backtrace = std::backtrace::Backtrace::capture();
            emit!(
                &self.inner.sink,
                ContextThreadMismatch {
                    name: ThreadName::new(thread.name().unwrap_or_default()),
                    id: thread.id().into(),
                    backtrace: BacktraceText(backtrace.to_string()),
                }
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[cfg_attr(test, mutants::skip)]
    #[expect(clippy::unused_self, reason = "Optimized away on release builds")]
    #[inline(always)]
    fn validate(&self) {
        // Disabled on release builds
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
        inner.sink.relocate(Some(&source), destination);
    }
}

impl Context {
    pub(crate) fn new(dispatcher: DispatcherClient, clock: Clock, io: Io, thread: Thread, sink: Sink) -> Self {
        Self {
            inner: Arc::new(ContextInner {
                scheduler: Scheduler::new(dispatcher.clone(), thread.clone()),
                control: RuntimeControl::new(dispatcher),
                thread,
                clock,
                io,
                sink,
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

impl AsRef<Sink> for Context {
    #[inline]
    fn as_ref(&self) -> &Sink {
        self.sink()
    }
}
