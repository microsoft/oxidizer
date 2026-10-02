// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::arc::Arc;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::blocking_worker::BlockingWorker;
use crate::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::task::Builtins;
use crate::task::join::JoinHandle;

/// A worker-bound handle for submitting async and blocking tasks.
///
/// Get it from [`Builtins::scheduler`] to keep child tasks on the same worker.
/// Clones keep that association but do not keep the runtime running. Use
/// [`RuntimeScheduler`](crate::task::RuntimeScheduler) to submit work from
/// synchronous code.
///
/// [`spawn`](Self::spawn) creates its future on the worker, so the future can
/// keep non-[`Send`] state across awaits. Factory captures and results must
/// still be `Send`, even when called on the same worker. Use
/// [`LocalTaskScheduler`](crate::task::LocalTaskScheduler) for non-`Send` captures
/// or results already on a worker.
///
/// [`spawn_anywhere`](Self::spawn_anywhere) lets the runtime place new work and
/// relocates a [`ThreadAware`] value there. If this scheduler is itself
/// relocated, it rebinds only to a ready worker of its own runtime.
///
/// Once shutdown starts, submissions return an immediately ready
/// [`JoinError`](crate::task::JoinError) without invoking the factory. Await
/// required results before shutdown; dropping a join does not cancel its task.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "rt"))]
/// #[arty::main]
/// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
///     let scheduler = cx.scheduler().clone();
///     let task = scheduler.spawn(async |_| 42);
///     assert_eq!(task.await?, 42);
///     Ok(())
/// }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[derive(Debug, Clone)]
pub struct TaskScheduler {
    pub(crate) dispatcher: DispatcherClient,
    binding: Binding,
}

#[derive(Debug, Clone)]
struct Binding {
    thread: Thread,
    worker_index: WorkerIndex,
    blocking_worker: Arc<BlockingWorker>,
}

impl TaskScheduler {
    pub(crate) fn new(dispatcher: DispatcherClient, current: Thread) -> Self {
        let worker_index = dispatcher
            .worker_index(current.id())
            .expect("scheduler thread must be registered with its dispatcher");
        let blocking_worker = dispatcher.blocking_worker(worker_index);
        Self {
            dispatcher,
            binding: Binding {
                thread: current,
                worker_index,
                blocking_worker,
            },
        }
    }

    pub(crate) fn current_worker_index(&self) -> WorkerIndex {
        self.binding.worker_index
    }

    pub(crate) fn resolve_worker_index(&self, thread: &Thread) -> Option<WorkerIndex> {
        self.dispatcher
            .owns(thread)
            .then(|| self.dispatcher.worker_index(thread.id()))
            .flatten()
    }

    pub(crate) fn relocate_to_worker(&mut self, destination: &Thread, worker_index: WorkerIndex) {
        self.binding = Binding {
            thread: destination.clone(),
            worker_index,
            blocking_worker: self.dispatcher.blocking_worker(worker_index),
        };
    }

    /// Submits an async task to this scheduler's worker.
    ///
    /// The factory receives [`Builtins`] and creates the future on that worker,
    /// so the future need not be [`Send`]. Pass a factory, not an already-created
    /// future.
    ///
    /// Captured values and returned results are not automatically relocated.
    /// Use [`spawn_anywhere`](Self::spawn_anywhere) to relocate an explicit payload.
    /// Factory and future panics are reported through the [`JoinHandle`].
    ///
    /// Tasks must not block their async worker. Use
    /// [`spawn_blocking`](Self::spawn_blocking) for synchronous blocking calls.
    ///
    /// # Examples
    ///
    /// Create non-`Send` state on the worker and retain it across a delay:
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    ///     use std::rc::Rc;
    ///     use std::time::Duration;
    ///
    ///     let answer = cx
    ///         .scheduler()
    ///         .spawn(|child| {
    ///             let value = Rc::new(42);
    ///             async move {
    ///                 child.clock().delay(Duration::from_millis(1)).await;
    ///                 *value
    ///             }
    ///         })
    ///         .await?;
    ///     assert_eq!(answer, 42);
    ///     Ok(())
    /// }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.dispatcher.spawn_on_worker(self.binding.worker_index, future_factory)
    }

    /// Submits a task to a worker chosen by the runtime and relocates its payload.
    ///
    /// On the chosen worker, relocates `data` from this scheduler's worker
    /// before calling `f`. The runtime may choose the same worker.
    ///
    /// `f` is a function pointer: pass values as `data` instead of capturing
    /// them. The result must implement [`ThreadAware`] and is not relocated.
    ///
    /// # Examples
    ///
    /// Relocate an existing set of capabilities to the selected worker:
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    ///     let answer = cx
    ///         .scheduler()
    ///         .spawn_anywhere(cx.clone(), |moved| async move {
    ///             assert_eq!(moved.thread().id(), std::thread::current().id());
    ///             assert!(moved.local_scheduler().is_some());
    ///             42
    ///         })
    ///         .await?;
    ///     assert_eq!(answer, 42);
    ///     Ok(())
    /// }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    ///
    /// A `Send` result without `ThreadAware` is not accepted:
    ///
    /// ```compile_fail
    /// use arty::runtime::Runtime;
    ///
    /// struct SendOnly;
    /// let runtime = Runtime::new().unwrap();
    /// let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    /// scheduler.spawn_anywhere((), |()| async { SendOnly });
    /// ```
    pub fn spawn_anywhere<D, F, R>(&self, data: D, f: fn(D) -> F) -> JoinHandle<R>
    where
        D: ThreadAware + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        let source = self.binding.thread.clone();
        self.dispatcher.spawn(async move |cx| {
            let mut data = data;
            data.relocate(Some(&source), cx.thread());
            f(data).await
        })
    }

    /// Submits one task to every worker and relocates a clone of its payload.
    ///
    /// Clones `data` for each worker and relocates each clone from this
    /// scheduler's worker before calling `f`. Other submissions do not affect
    /// which workers receive a task.
    ///
    /// Returns joins in the runtime's worker order. Futures need not be [`Send`];
    /// their results must implement [`ThreadAware`] (which includes `Send`).
    /// Shutdown rejects submissions without invoking `f`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    ///     let tasks = cx
    ///         .scheduler()
    ///         .spawn_everywhere(cx.clone(), |worker| async move { worker.thread().id() });
    ///     for task in tasks {
    ///         let _worker = task.await?;
    ///     }
    ///     Ok(())
    /// }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    ///
    /// Every result must implement `ThreadAware`:
    ///
    /// ```compile_fail
    /// use arty::runtime::Runtime;
    ///
    /// struct SendOnly;
    /// let runtime = Runtime::new().unwrap();
    /// let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    /// scheduler.spawn_everywhere((), |()| async { SendOnly });
    /// ```
    pub fn spawn_everywhere<D, F, R>(&self, data: D, f: fn(D) -> F) -> Vec<JoinHandle<R>>
    where
        D: ThreadAware + Clone + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        let source = &self.binding.thread;
        let tasks = self.dispatcher.spawn_everywhere(|| {
            let source = source.clone();
            let mut data = data.clone();
            async move |cx| {
                data.relocate(Some(&source), cx.thread());
                f(data).await
            }
        });
        drop(data);
        tasks
    }

    /// Submits a synchronous callback to a blocking-task pool.
    ///
    /// Use this for synchronous I/O or library calls that would otherwise prevent
    /// an async worker from polling tasks and advancing timers. The callback
    /// runs on a separate pool thread; its panic is reported through the join.
    ///
    /// Uses the associated worker's pool. Configure sharing and limits with
    /// [`BlockingPoolPolicy`](crate::runtime::BlockingPoolPolicy).
    ///
    /// Shutdown rejects new callbacks and cancels queued ones before invocation.
    /// Already-running callbacks cannot be interrupted and are allowed to finish.
    /// Pool sizing targets blocking calls, not sustained CPU-parallel workloads.
    ///
    /// # Examples
    ///
    /// Read a file without blocking the async worker:
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// #[arty::main]
    /// async fn main(
    ///     cx: arty::task::Builtins,
    /// ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ///     let contents = cx
    ///         .scheduler()
    ///         .spawn_blocking(|| std::fs::read_to_string("settings.toml"))
    ///         .await??;
    ///     println!("{contents}");
    ///     Ok(())
    /// }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    ///
    /// The first `?` handles task failure; the second handles the file's I/O error.
    pub fn spawn_blocking<B, R>(&self, body: B) -> JoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if self.dispatcher.is_shutting_down() {
            return JoinHandle::shutdown();
        }
        self.binding.blocking_worker.spawn_blocking(body)
    }

    #[cfg(test)]
    pub(crate) fn blocking_worker(&self) -> &Arc<BlockingWorker> {
        &self.binding.blocking_worker
    }
}

impl ThreadAware for TaskScheduler {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.binding.thread == *destination {
            return;
        }
        if let Some(index) = self.resolve_worker_index(destination) {
            self.relocate_to_worker(destination, index);
        }
    }
}

impl AsRef<Self> for TaskScheduler {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn assert_send_sync() {
        static_assertions::assert_impl_all!(TaskScheduler: Send, Sync);
    }

    #[cfg(not(miri))]
    #[test]
    fn a_foreign_owner_cannot_rebind_a_registered_thread_id() {
        struct RelocationSource(Option<Thread>);

        impl ThreadAware for RelocationSource {
            fn relocate(&mut self, source: Option<&Thread>, _: &Thread) {
                self.0 = source.cloned();
            }
        }

        let runtime = crate::runtime::Runtime::builder()
            .processor_count(crate::runtime::ProcessorCount::exactly(1))
            .build()
            .unwrap();
        let (source, mut scheduler) = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { (cx.thread().clone(), cx.scheduler().clone()) })
            .wait()
            .unwrap();
        let foreign = thread_aware::ThreadBuilder::default().build(source.id());
        assert_ne!(source.owner(), foreign.owner());

        scheduler.relocate(None, &foreign);
        let scheduler: &TaskScheduler = scheduler.as_ref();
        let actual = scheduler
            .spawn_anywhere(RelocationSource(None), |probe| async move { probe.0 })
            .wait()
            .unwrap();
        assert_eq!(actual, Some(source));
    }
}
