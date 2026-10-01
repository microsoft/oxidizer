// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task capabilities, their guarded thread-affine state, and relocation.

use std::sync::Arc;

use many_cpus::ProcessorSet;
use observed::Sink;
#[cfg(debug_assertions)]
use observed::emit;
use thread_aware::{Thread, ThreadAware};
use tick::{Clock, SimpleClock};

use crate::runtime::context::{RuntimeBuiltins, SharedState};
#[cfg(debug_assertions)]
use crate::runtime::telemetry::events::{BacktraceText, BuiltinsThreadMismatch, ThreadName};
use crate::task::local::{LocalTaskBinding, LocalTaskScheduler};
use crate::task::scheduler::TaskScheduler;

/// Worker-bound services supplied to an asynchronous task.
///
/// Use [`scheduler`](Self::scheduler) for child tasks,
/// [`local_scheduler`](Self::local_scheduler) for non-[`Send`] captures and results,
/// and [`clock`](Self::clock) for delays and timeouts. Runtime task factories
/// receive this value by ownership.
///
/// When starting work on another worker, prefer the `Builtins` supplied to that
/// task's factory. Cloning or moving an existing value preserves its association;
/// it does not keep the original runtime running.
///
/// [`TaskScheduler::spawn_anywhere`] explicitly relocates a payload. For `Builtins`,
/// relocation to an initialized worker of the same runtime rebinds all services
/// together. A foreign or unregistered destination leaves them unchanged.
/// Local scheduling is available only on the value's associated worker.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::main]
/// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
///     let answer = cx
///         .scheduler()
///         .spawn(async |child| {
///             child
///                 .clock()
///                 .delay(std::time::Duration::from_millis(1))
///                 .await;
///             42
///         })
///         .await?;
///     assert_eq!(answer, 42);
///     Ok(())
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
#[derive(Debug, Clone)]
pub struct Builtins {
    pub(crate) scheduler: TaskScheduler,
    thread: Thread,
    inner: Arc<InnerBuiltins>,
    pub(crate) shared_state: SharedState,
    clock: Clock,
    sink: Sink,
}

impl Builtins {
    /// Returns a scheduler that creates tasks on the associated worker.
    ///
    /// Cloning the scheduler or using it from another thread preserves that
    /// worker association. Use [`TaskScheduler::spawn_anywhere`] to distribute
    /// new work across workers instead.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    ///     let parent_thread = cx.thread().id();
    ///     let child_thread = cx
    ///         .scheduler()
    ///         .spawn(async |child| child.thread().id())
    ///         .await?;
    ///     assert_eq!(child_thread, parent_thread);
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn scheduler(&self) -> &TaskScheduler {
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
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) {
    ///     assert_eq!(cx.thread().id(), std::thread::current().id());
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
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
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) {
    ///     let duration = std::time::Duration::from_millis(1);
    ///     let watch = cx.clock().stopwatch();
    ///     cx.clock().delay(duration).await;
    ///     assert!(watch.elapsed() >= duration);
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
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
    /// # #[cfg(feature = "macros")]
    /// #[observed::event("app.task.started")]
    /// #[info("task started")]
    /// struct TaskStarted;
    ///
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) {
    ///     observed::emit!(cx.sink(), TaskStarted);
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
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
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    ///     use std::rc::Rc;
    ///
    ///     let local = cx
    ///         .local_scheduler()
    ///         .expect("the task runs on its associated worker");
    ///     let result = local.spawn(async || Rc::new(42)).await?;
    ///     assert_eq!(*result, 42);
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn local_scheduler(&self) -> Option<LocalTaskScheduler> {
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

impl AsRef<TaskScheduler> for Builtins {
    fn as_ref(&self) -> &TaskScheduler {
        self.scheduler()
    }
}

impl AsRef<Clock> for Builtins {
    fn as_ref(&self) -> &Clock {
        self.clock()
    }
}

impl AsRef<SimpleClock> for Builtins {
    fn as_ref(&self) -> &SimpleClock {
        self.clock().as_ref()
    }
}

impl AsRef<Sink> for Builtins {
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::fmt::Debug;
    #[cfg(not(miri))]
    use std::thread;

    #[cfg(not(miri))]
    use futures::future::join_all;
    #[cfg(all(debug_assertions, not(miri)))]
    use many_cpus::SystemHardware;
    #[cfg(all(debug_assertions, not(miri)))]
    use observed_testing::{TEST_ID, test_emitter};

    use super::*;
    #[cfg(not(miri))]
    use crate::runtime::Runtime;
    #[cfg(not(miri))]
    use crate::runtime::config::ProcessorCount;

    #[test]
    fn assert_builtin_traits() {
        static_assertions::assert_impl_all!(Builtins: AsRef<TaskScheduler>, AsRef<Clock>, Send, Sync, Clone, Debug);
    }

    #[cfg(not(miri))]
    #[test]
    fn borrowed_services_are_the_worker_services() {
        Runtime::builder()
            .processor_count(ProcessorCount::exactly(1))
            .build()
            .unwrap()
            .scheduler()
            .block_on(async |cx| {
                let scheduler: &TaskScheduler = cx.as_ref();
                let clock: &Clock = cx.as_ref();
                let simple_clock: &SimpleClock = cx.as_ref();
                let sink: &Sink = cx.as_ref();
                assert!(std::ptr::eq(scheduler, cx.scheduler()));
                assert!(std::ptr::eq(clock, cx.clock()));
                assert!(std::ptr::eq(simple_clock, cx.clock().as_ref()));
                assert!(std::ptr::eq(sink, cx.sink()));
                assert_eq!(scheduler.spawn(async |_| 42).await.unwrap(), 42);
            })
            .unwrap();
    }

    #[cfg(all(debug_assertions, not(miri)))]
    #[test]
    fn validation_accepts_associated_worker() {
        let (sink, processor) = test_emitter(TEST_ID);
        Runtime::builder()
            .processor_count(ProcessorCount::exactly(1))
            .sink(sink)
            .build()
            .unwrap()
            .scheduler()
            .block_on(async |cx| {
                let _ = cx.thread();
            })
            .unwrap();

        assert_eq!(
            processor
                .events()
                .iter()
                .filter(|event| { event.name() == "arty.rt.builtins.thread_mismatch" })
                .count(),
            0,
        );
    }

    #[cfg(all(debug_assertions, not(miri)))]
    #[test]
    fn validation_follows_relocation_with_known_source() {
        validation_follows_accepted_worker(true);
    }

    #[cfg(all(debug_assertions, not(miri)))]
    #[test]
    fn validation_follows_relocation_with_unknown_source() {
        validation_follows_accepted_worker(false);
    }

    #[cfg(all(debug_assertions, not(miri)))]
    #[cfg_attr(test, mutants::skip)]
    fn validation_follows_accepted_worker(known_source: bool) {
        if SystemHardware::current().processors().len() < 2 {
            eprintln!("requires two runtime workers to exercise cross-worker validation");
            return;
        }
        let (sink, processor) = test_emitter(TEST_ID);
        let runtime = Runtime::builder()
            .processor_count(ProcessorCount::exactly(2))
            .sink(sink)
            .build()
            .unwrap();
        let workers: Vec<_> = (0..2)
            .map(|_| {
                runtime.scheduler().spawn_anywhere(async |cx| {
                    let scheduler = cx.scheduler().clone();
                    (cx, scheduler)
                })
            })
            .map(|handle| handle.wait().unwrap())
            .collect();
        let mut builtins = workers[0].0.clone();
        let source = builtins.thread.clone();
        workers[1]
            .1
            .spawn(async move |cx| {
                let _ = builtins.thread();
                builtins.relocate(known_source.then_some(&source), &cx.thread);
                let _ = builtins.thread();
            })
            .wait()
            .unwrap();
        runtime.stop().unwrap();

        assert_eq!(
            processor
                .events()
                .iter()
                .filter(|event| { event.name() == "arty.rt.builtins.thread_mismatch" })
                .count(),
            1,
        );
    }

    #[cfg(not(miri))]
    #[test]
    fn worker_services_are_ready_before_spawning() {
        let count = many_cpus::SystemHardware::current().processors().len().min(2);
        let runtime = Runtime::builder().processor_count(ProcessorCount::at_most(2)).build().unwrap();
        let (actual, expected) = runtime
            .scheduler()
            .block_on(async move |cx| {
                let scheduler = cx.scheduler();
                let tasks: Vec<_> = (0..count)
                    .map(|_| {
                        scheduler.spawn_anywhere(cx.clone(), |worker: Builtins| async move {
                            (worker.thread().id() == thread::current().id(), worker.local_scheduler().is_some())
                        })
                    })
                    .collect();
                let expected = vec![(true, true); tasks.len()];
                (join_all(tasks).await.into_iter().map(Result::unwrap).collect::<Vec<_>>(), expected)
            })
            .unwrap();

        assert_eq!(actual, expected);
    }
}
