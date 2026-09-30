// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime capabilities, their guarded thread-affine state, and relocation.

pub(in crate::runtime) mod init;
pub(crate) mod operations;

use std::sync::Arc;

use many_cpus::ProcessorSet;
use observed::Sink;
#[cfg(debug_assertions)]
use observed::emit;
use thread_aware::{Thread, ThreadAware};
use tick::{Clock, SimpleClock};

use crate::runtime::context::init::{RuntimeBuiltins, SharedState};
use crate::runtime::context::operations::RuntimeOperations;
#[cfg(debug_assertions)]
use crate::runtime::telemetry::events::{BacktraceText, BuiltinsThreadMismatch, ThreadName};
use crate::task::local::{LocalTaskBinding, LocalTaskScheduler};
use crate::task::scheduler::TaskScheduler;

/// Owned runtime capabilities associated with one worker.
///
/// Runtime callbacks receive this value by ownership. It provides a worker-bound
/// scheduler, clock, telemetry sink, and runtime operations. Cloning preserves
/// that association and does not keep the runtime running.
///
/// [`TaskScheduler::spawn_anywhere`] can explicitly relocate this value as its
/// payload. Relocation to an initialized worker of the owning runtime rebinds
/// its capabilities together. A foreign or unregistered destination preserves
/// the original association; it does not move services into another runtime.
///
/// The value is portable, but its local scheduler is available only on its
/// associated worker. See the [documentation guides](crate#documentation)
/// for examples and the distinction between cloning, relocation, and execution.
#[derive(Debug, Clone)]
pub struct Builtins {
    scheduler: TaskScheduler,
    runtime_operations: RuntimeOperations,
    thread: Thread,
    inner: Arc<InnerBuiltins>,
    shared_state: SharedState,
    clock: Clock,
    sink: Sink,
}

impl Builtins {
    /// Returns a scheduler bound to this capability's worker.
    #[must_use]
    #[inline]
    pub fn scheduler(&self) -> &TaskScheduler {
        self.validate();

        &self.scheduler
    }

    /// Returns the associated worker's coordinate, including its NUMA locality.
    ///
    /// This describes the association, not necessarily the thread executing the caller.
    #[must_use]
    #[inline]
    pub fn thread(&self) -> &Thread {
        self.validate();

        &self.thread
    }

    /// Returns the worker's clock for time queries, delays, and timeouts.
    #[must_use]
    #[inline]
    pub fn clock(&self) -> &Clock {
        self.validate();

        &self.clock
    }

    /// Returns the [`Sink`] associated with this runtime.
    ///
    /// If no sink is configured in [`crate::runtime::RuntimeBuilder`], this returns a noop sink.
    #[must_use]
    #[inline]
    pub fn sink(&self) -> &Sink {
        self.validate();

        &self.sink
    }

    /// Returns a local scheduler when called on the associated worker.
    ///
    /// Returns an owned, thread-confined token. Portable `Builtins` values do not
    /// own the executor's non-`Send` state, including when dropped on another thread.
    ///
    /// Returns `None` outside that worker's local context. A scheduler already
    /// obtained there remains thread-confined; it cannot be sent to another thread.
    ///
    /// With the `macros` feature:
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// # #[cfg(feature = "macros")]
    /// use arty::runtime::Builtins;
    ///
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: Builtins) {
    ///     let local = cx
    ///         .local_scheduler()
    ///         .expect("the task runs on its associated worker");
    ///     let result = local.spawn(async || Rc::new(42)).await;
    ///     assert_eq!(*result, 42);
    ///
    ///     let portable = cx.clone();
    ///     let unavailable = cx
    ///         .scheduler()
    ///         .spawn_blocking(move || portable.local_scheduler().is_none())
    ///         .await;
    ///     assert!(unavailable);
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
    /// ```
    #[must_use]
    #[inline]
    pub fn local_scheduler(&self) -> Option<LocalTaskScheduler> {
        self.inner.local_task_binding.local_scheduler()
    }

    /// Returns runtime shutdown and processor-pinning operations.
    ///
    /// These operations are available from any thread. Relocation within the owning
    /// runtime rebinds them to the destination worker.
    #[must_use]
    #[inline]
    pub fn runtime_operations(&self) -> &RuntimeOperations {
        &self.runtime_operations
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
        self.runtime_operations.relocate_to_worker(destination, inner.processor_set.clone());
        self.inner = inner;
        self.thread = destination.clone();
        self.clock.relocate(Some(&source), destination);
        self.sink.relocate(Some(&source), destination);
    }
}

impl Builtins {
    pub(in crate::runtime) fn sync_init(shared_state: &SharedState, builtins: RuntimeBuiltins) -> Self {
        let thread = builtins.core.thread;
        let runtime_operations = RuntimeOperations::new(
            builtins.core.dispatcher,
            thread.clone(),
            builtins.core.processor_set.clone(),
            Arc::clone(shared_state),
        );

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
            runtime_operations,
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
pub(in crate::runtime) struct InnerBuiltins {
    processor_set: ProcessorSet,
    local_task_binding: LocalTaskBinding,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::fmt::Debug;
    #[cfg(not(miri))]
    use std::num::NonZeroUsize;
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
    use crate::runtime::config::ProcessorCount;
    #[cfg(not(miri))]
    use crate::runtime::handle::Runtime;
    #[cfg(all(debug_assertions, not(miri)))]
    use crate::task::join::JoinHandle;

    #[test]
    fn assert_builtin_traits() {
        static_assertions::assert_impl_all!(Builtins: AsRef<TaskScheduler>, AsRef<Clock>, Send, Sync, Clone, Debug);
    }

    #[cfg(not(miri))]
    #[test]
    fn borrowed_services_are_the_worker_services() {
        Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
            .build()
            .unwrap()
            .run(async |cx| {
                let scheduler: &TaskScheduler = cx.as_ref();
                let clock: &Clock = cx.as_ref();
                let simple_clock: &SimpleClock = cx.as_ref();
                let sink: &Sink = cx.as_ref();
                assert!(std::ptr::eq(scheduler, cx.scheduler()));
                assert!(std::ptr::eq(clock, cx.clock()));
                assert!(std::ptr::eq(simple_clock, cx.clock().as_ref()));
                assert!(std::ptr::eq(sink, cx.sink()));
                assert_eq!(scheduler.spawn(async |_| 42).await, 42);
            });
    }

    #[cfg(all(debug_assertions, not(miri)))]
    #[test]
    fn validation_accepts_associated_worker() {
        let (sink, processor) = test_emitter(TEST_ID);
        Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
            .sink(sink)
            .build()
            .unwrap()
            .run(async |cx| {
                let _ = cx.thread();
            });

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
            .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()))
            .sink(sink)
            .build()
            .unwrap();
        let workers: Vec<_> = (0..2)
            .map(|_| {
                runtime.task_scheduler().spawn(async |cx| {
                    let scheduler = cx.scheduler().clone();
                    (cx, scheduler)
                })
            })
            .map(JoinHandle::wait)
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
            .wait();
        runtime.stop();
        runtime.wait();

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
        let runtime = Runtime::builder()
            .processor_count(ProcessorCount::at_most(NonZeroUsize::new(2).unwrap()))
            .build()
            .unwrap();
        let scheduler = runtime.task_scheduler();
        let (actual, expected) = runtime.run(async move |cx| {
            let tasks: Vec<_> = (0..count)
                .map(|_| {
                    scheduler.spawn_anywhere(cx.clone(), |worker: Builtins| async move {
                        (worker.thread().id() == thread::current().id(), worker.local_scheduler().is_some())
                    })
                })
                .collect();
            let expected = vec![(true, true); tasks.len()];
            (join_all(tasks).await, expected)
        });

        assert_eq!(actual, expected);
    }
}
