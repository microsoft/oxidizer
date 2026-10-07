// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker services and thread-aware relocation.

use many_cpus::ProcessorSet;
use observed::Sink;
#[cfg(debug_assertions)]
use observed::emit;
use performables::arc::Arc;
use thread_aware::{Thread, ThreadAware};
use tick::Clock;

use crate::runtime::context::{RuntimeBuiltins, SharedState};
#[cfg(debug_assertions)]
use crate::runtime::telemetry::events::{BacktraceText, BuiltinsThreadMismatch, ThreadName};
use crate::task::local::{LocalScheduler, LocalTaskBinding};
use crate::task::scheduler::Scheduler;

/// Services supplied to an async task on its worker.
///
/// Use [`scheduler`](Self::scheduler) for child tasks,
/// [`local_scheduler`](Self::local_scheduler) for non-[`Send`] captures and results,
/// and [`clock`](Self::clock) for delays and timeouts. Runtime task factories
/// receive this value by ownership.
///
/// New tasks receive their own `Builtins`. Cloning or moving an existing value
/// does not change its worker or keep its runtime running.
///
/// [`Scheduler::spawn_anywhere`] can relocate an existing `Builtins` to
/// another ready worker of the same runtime. A worker from another runtime, or
/// one that is not ready, leaves its association unchanged. Local scheduling
/// is available only on the associated worker.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// # #[arty::main]
/// # async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
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
pub struct Builtins {
    pub(crate) scheduler: Scheduler,
    thread: Thread,
    inner: Arc<InnerBuiltins>,
    pub(crate) shared_state: SharedState,
    clock: Clock,
    sink: Sink,
}

impl Builtins {
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
    /// # async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
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

        &self.scheduler
    }

    /// Returns the associated worker's coordinate, including its NUMA locality.
    ///
    /// This describes where the services belong, not necessarily the thread
    /// executing the caller. Moving `Builtins` without relocation leaves this
    /// coordinate unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Builtins) {
    /// assert_eq!(cx.thread().id(), std::thread::current().id());
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn thread(&self) -> &Thread {
        self.validate();

        &self.thread
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
    /// # async fn main(cx: arty::task::Builtins) {
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

        &self.clock
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
    /// # async fn main(cx: arty::task::Builtins) {
    /// observed::emit!(cx.sink(), TaskStarted);
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn sink(&self) -> &Sink {
        self.validate();

        &self.sink
    }

    /// Returns a local scheduler when called on the associated worker.
    ///
    /// Use it to share non-[`Send`] state between tasks on the same worker.
    /// The returned scheduler is owned, but cannot be sent to another thread.
    ///
    /// Returns `None` when called outside this value's associated worker.
    /// Carrying `Builtins` to another thread does not grant local scheduling
    /// access there.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    /// use std::rc::Rc;
    ///
    /// let local = cx
    ///     .local_scheduler()
    ///     .expect("the task runs on its associated worker");
    /// let result = local.spawn(async || Rc::new(42)).await?;
    /// assert_eq!(*result, 42);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn local_scheduler(&self) -> Option<LocalScheduler> {
        self.inner.local_task_binding.local_scheduler()
    }

    #[cfg_attr(test, mutants::skip)]
    #[cfg(debug_assertions)]
    fn validate(&self) {
        let thread = std::thread::current();
        if thread.id() != self.thread.id() {
            let backtrace = std::backtrace::Backtrace::capture();
            emit!(
                &self.sink,
                BuiltinsThreadMismatch {
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

impl ThreadAware for Builtins {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.thread == *destination {
            return;
        }

        // The scheduler validates runtime ownership before resolving a worker.
        let Some(worker_index) = self.scheduler.resolve_worker_index(destination) else {
            return;
        };
        let Some(inner) = self
            .shared_state
            .get(usize::from(worker_index))
            .expect("registered runtime worker must have a shared-state slot")
            .get()
            .cloned()
        else {
            return;
        };
        let source = self.thread.clone();

        // Reuse the validated slot index to keep all runtime services on the same worker.
        self.scheduler.relocate_to_worker(destination, worker_index);
        self.inner = inner;
        self.thread = destination.clone();
        self.clock.relocate(Some(&source), destination);
        self.sink.relocate(Some(&source), destination);
    }
}

impl Builtins {
    pub(crate) fn sync_init(shared_state: &SharedState, builtins: RuntimeBuiltins) -> Self {
        let thread = builtins.core.thread;

        let inner = Arc::new(InnerBuiltins {
            local_task_binding: builtins.core.local_scheduler,
            processor_set: builtins.core.processor_set,
        });

        // Each worker initializes its own thread once. Write-once storage prevents
        // thread-bound runtime state from being replaced after it becomes observable.
        shared_state
            .get(usize::from(builtins.task_scheduler.current_worker_index()))
            .expect("registered runtime worker must have a shared-state slot")
            .set(Arc::clone(&inner))
            .expect("each runtime thread is initialized exactly once");

        Self {
            scheduler: builtins.task_scheduler,
            thread,
            inner,
            shared_state: Arc::clone(shared_state),
            clock: builtins.clock,
            sink: builtins.core.sink,
        }
    }
}

impl AsRef<Scheduler> for Builtins {
    #[inline]
    fn as_ref(&self) -> &Scheduler {
        self.scheduler()
    }
}

impl AsRef<Clock> for Builtins {
    #[inline]
    fn as_ref(&self) -> &Clock {
        self.clock()
    }
}

impl AsRef<Sink> for Builtins {
    #[inline]
    fn as_ref(&self) -> &Sink {
        self.sink()
    }
}

/// Immutable worker-local service bundle published once into its runtime's shared slots.
#[derive(Debug)]
pub(crate) struct InnerBuiltins {
    pub(crate) processor_set: ProcessorSet,
    local_task_binding: LocalTaskBinding,
}
