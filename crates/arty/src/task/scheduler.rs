// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{TaskOptions, TaskPlacement};
use performables::arc::Arc;
use thread_aware::{Thread, ThreadAware};

use crate::runtime::dispatch::{DispatcherClient, WorkerIndex};
use crate::runtime::worker::asynchronous;
use crate::runtime::worker::blocking::Worker;
use crate::task::Context;
use crate::task::handle::JoinHandle;
use crate::task::preparation::DeferredTask;

/// A worker-bound handle for submitting async and blocking tasks.
///
/// Get it from [`Context::scheduler`] to keep child tasks on the same worker.
/// Clones keep that association but do not keep the runtime running. Use
/// [`RuntimeScheduler`](crate::task::RuntimeScheduler) to submit work from
/// synchronous code.
///
/// [`spawn`](Self::spawn) creates its future on the worker, so the future can
/// keep non-[`Send`] state across awaits. Factory captures and results must
/// still be `Send`, even when called on the same worker.
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
/// # #[arty::main]
/// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
/// let scheduler = cx.scheduler().clone();
/// let task = scheduler.spawn(async |_| 42);
/// assert_eq!(task.await?, 42);
/// # Ok(())
/// # }
/// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
/// ```
#[derive(Debug, Clone)]
pub struct Scheduler {
    pub(crate) dispatcher: DispatcherClient,
    binding: Binding,
}

#[derive(Debug, Clone)]
struct Binding {
    thread: Thread,
    worker_index: WorkerIndex,
    blocking_worker: Arc<Worker>,
}

impl Scheduler {
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
    /// The factory receives [`Context`] and creates the future on that worker,
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
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
    /// use std::rc::Rc;
    /// use std::time::Duration;
    ///
    /// let answer = cx
    ///     .scheduler()
    ///     .spawn(|child| {
    ///         let value = Rc::new(42);
    ///         async move {
    ///             child.clock().delay(Duration::from_millis(1)).await;
    ///             *value
    ///         }
    ///     })
    ///     .await?;
    /// assert_eq!(answer, 42);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    pub fn spawn<FF, F, R>(&self, future_factory: FF) -> JoinHandle<R>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        if let Some(join_handle) = JoinHandle::try_from_shutdown(self.dispatcher.shutdown()) {
            return join_handle;
        }
        let future_factory = match self.spawn_on_current_worker(future_factory) {
            Ok(join_handle) => return join_handle,
            Err(future_factory) => future_factory,
        };
        self.dispatcher.spawn_on_worker(self.binding.worker_index, future_factory)
    }

    fn spawn_on_current_worker<FF, F, R>(&self, future_factory: FF) -> Result<JoinHandle<R>, FF>
    where
        FF: FnOnce(Context) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        asynchronous::LocalWorker::with_scheduler(|current| {
            let Some(current) = current else {
                return Err(future_factory);
            };
            if current.thread() != &self.binding.thread {
                return Err(future_factory);
            }
            let sink = current.context().sink();
            let parent_task_enrichment = sink.transfer_context();
            let context = current.context().clone();
            let (handle, result_tx) = current.results().handle();
            let inner = DeferredTask::new(future_factory, context);
            let options = TaskOptions::observed(parent_task_enrichment, TaskPlacement::SameThread);
            current.tasks().add(inner, options, result_tx);
            self.dispatcher.wake_worker(self.binding.worker_index);
            Ok(handle)
        })
    }

    /// Submits a task to a worker chosen by the runtime and relocates its payload.
    ///
    /// On the chosen worker, relocates `data` from this scheduler's worker
    /// before calling `f`. The runtime may choose the same worker.
    ///
    /// `f` is a function pointer: pass values as `data` instead of capturing
    /// them. The result must implement [`ThreadAware`]. When its join completes
    /// on an Arty worker, the result is relocated from the task's worker to the
    /// joining worker before being returned. Normally this is the source worker;
    /// moving the join to another worker changes the return destination.
    /// Joining outside an Arty worker does not relocate the result.
    ///
    /// # Examples
    ///
    /// Relocate an existing set of capabilities to the selected worker:
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
    /// let answer = cx
    ///     .scheduler()
    ///     .spawn_anywhere(cx.clone(), |moved| async move {
    ///         assert_eq!(moved.thread().id(), std::thread::current().id());
    ///         42
    ///     })
    ///     .await?;
    /// assert_eq!(answer, 42);
    /// # Ok(())
    /// # }
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
        self.dispatcher.spawn_relocating(async move |cx| {
            let mut data = data;
            data.relocate(Some(&source), cx.thread());
            f(data).await
        })
    }

    /// Submits a task to `destination` and relocates its payload there.
    ///
    /// The destination must be a worker coordinate returned by this runtime's
    /// [`RuntimeControl::workers`](crate::runtime::RuntimeControl::workers).
    /// Before calling `f`, Arty relocates `data` from this scheduler's worker to
    /// the destination.
    ///
    /// Futures need not be [`Send`]. Results must implement [`ThreadAware`].
    /// When the join completes on an Arty worker, the result is relocated from
    /// `destination` to the joining worker, normally this scheduler's worker.
    /// Joining outside Arty does not relocate the result. Shutdown or an
    /// unsupported destination rejects the submission without relocating
    /// `data` or invoking `f`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(
    /// #     cx: arty::task::Context,
    /// # ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    /// let destination = cx
    ///     .control()
    ///     .workers()
    ///     .into_iter()
    ///     .next()
    ///     .ok_or_else(|| std::io::Error::other("runtime has no workers"))?;
    /// let task = cx
    ///     .scheduler()
    ///     .spawn_at(destination, cx.clone(), |worker| async move {
    ///         worker.thread().id()
    ///     });
    /// assert_eq!(task.await?, destination.id());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    ///
    /// Every result must implement `ThreadAware`:
    ///
    /// ```compile_fail
    /// use arty::runtime::Runtime;
    ///
    /// struct SendOnly;
    ///
    /// fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ///     let runtime = Runtime::new()?;
    ///     let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone())?;
    ///     let destination = runtime.scheduler().block_on(async |cx| {
    ///         cx.control()
    ///             .workers()
    ///             .into_iter()
    ///             .next()
    ///             .cloned()
    ///             .ok_or_else(|| std::io::Error::other("runtime has no workers"))
    ///     })??;
    ///     scheduler.spawn_at(&destination, (), |()| async { SendOnly });
    ///     Ok(())
    /// }
    /// ```
    pub fn spawn_at<D, F, R>(&self, destination: &Thread, data: D, f: fn(D) -> F) -> JoinHandle<R>
    where
        D: ThreadAware + 'static,
        F: Future<Output = R> + 'static,
        R: ThreadAware + 'static,
    {
        if let Some(join_handle) = JoinHandle::try_from_shutdown(self.dispatcher.shutdown()) {
            return join_handle;
        }
        let Some(worker_index) = self.resolve_worker_index(destination) else {
            return JoinHandle::invalid_destination();
        };
        let source = self.binding.thread.clone();
        self.dispatcher.spawn_at_worker_relocating(worker_index, move |cx| {
            let mut data = data;
            async move {
                data.relocate(Some(&source), cx.thread());
                f(data).await
            }
        })
    }

    /// Submits a synchronous callback to a blocking-task pool.
    ///
    /// Use this for synchronous I/O or library calls that would otherwise prevent
    /// an async worker from polling tasks and advancing timers. The callback
    /// runs on a separate pool thread. A callback panic fails its join handle
    /// and follows the configured [`PanicPolicy`](crate::runtime::PanicPolicy).
    ///
    /// Uses the associated worker's pool. Configure sharing and limits with
    /// [`BlockingPoolPolicy`](crate::runtime::BlockingPoolPolicy).
    ///
    /// Shutdown rejects new callbacks and cancels queued ones before invocation.
    /// Already-running callbacks cannot be interrupted and are allowed to finish.
    /// Pool sizing targets blocking calls, not sustained CPU-parallel workloads.
    ///
    /// A callback must not synchronously poll another blocking handle from its
    /// own pool; direct attempts panic.
    /// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on)
    /// also rejects calls from blocking callbacks before task submission.
    ///
    /// # Examples
    ///
    /// Read a file without blocking the async worker:
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main]
    /// # async fn main(cx: arty::task::Context) -> Result<(), ohno::AppError> {
    /// let contents = cx
    ///     .scheduler()
    ///     .spawn_blocking(|| std::fs::read_to_string("settings.toml"))
    ///     .await??;
    /// println!("{contents}");
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    ///
    /// The first `?` handles task failure; the second handles the file's I/O error.
    pub fn spawn_blocking<B, R>(&self, body: B) -> JoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.binding.blocking_worker.spawn_blocking(body)
    }

    #[cfg(test)]
    pub(crate) fn blocking_worker(&self) -> &Arc<Worker> {
        &self.binding.blocking_worker
    }
}

impl ThreadAware for Scheduler {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        if self.binding.thread == *destination {
            return;
        }
        if let Some(index) = self.resolve_worker_index(destination) {
            self.relocate_to_worker(destination, index);
        }
    }
}

impl AsRef<Self> for Scheduler {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::cell::RefCell;
    use std::future::Future as _;
    use std::pin::pin;
    use std::sync::mpsc;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use arty_executor::ShutdownCause;

    use super::*;

    thread_local! {
        static SPAWN_ON_DROP: RefCell<Option<SpawnOnDrop>> = const { RefCell::new(None) };
    }

    struct SpawnOnDrop {
        scheduler: Scheduler,
        spawned: mpsc::Sender<JoinHandle<()>>,
    }

    impl Drop for SpawnOnDrop {
        fn drop(&mut self) {
            self.spawned.send(self.scheduler.spawn(async |_| {})).unwrap();
        }
    }

    #[test]
    fn worker_execution_registers_the_current_scheduler() {
        let runtime = crate::runtime::Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let registered = runtime
            .scheduler()
            .block_on(async |_| {
                assert!(asynchronous::LocalWorker::is_current());
                asynchronous::LocalWorker::has_scheduler_context()
            })
            .unwrap();

        assert!(registered);
        runtime.stop().unwrap();
    }

    #[test]
    fn dispatcher_shutdown_rejects_direct_submissions_immediately() {
        let runtime = crate::runtime::Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let rejected = runtime
            .scheduler()
            .block_on(async |cx| {
                _ = cx.scheduler().dispatcher.stop(ShutdownCause::default());
                let mut task = pin!(cx.scheduler().spawn(async |_| 42));

                matches!(
                    task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                    Poll::Ready(Err(error)) if error.is_shutdown()
                )
            })
            .unwrap();

        assert!(rejected);
        runtime.stop().unwrap();
    }

    #[test]
    fn worker_marker_remains_active_after_scheduler_context_is_cleared() {
        struct DropProbe(mpsc::Sender<(bool, bool)>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0
                    .send((
                        !asynchronous::LocalWorker::has_scheduler_context(),
                        asynchronous::LocalWorker::is_current(),
                    ))
                    .unwrap();
            }
        }

        let runtime = crate::runtime::Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (cleared_tx, cleared_rx) = mpsc::channel();

        runtime
            .scheduler()
            .block_on(async move |cx| {
                let task = cx.scheduler().spawn(move |_| async move {
                    started_tx.send(()).unwrap();
                    let _probe = DropProbe(cleared_tx);
                    std::future::pending::<()>().await;
                });
                drop(task);
            })
            .unwrap();

        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.stop().unwrap();
        assert_eq!(cleared_rx.recv_timeout(Duration::from_secs(5)).unwrap(), (true, true));
    }

    #[test]
    fn external_spawn_survives_scheduler_slot_teardown() {
        let runtime = crate::runtime::Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .build()
            .unwrap();
        let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
        let (spawned, received) = mpsc::channel();

        std::thread::spawn(move || {
            SPAWN_ON_DROP.with(|owner| {
                assert!(
                    owner
                        .replace(Some(SpawnOnDrop {
                            scheduler: scheduler.clone(),
                            spawned,
                        }))
                        .is_none()
                );
            });
            scheduler.spawn(async |_| {}).join().unwrap();
        })
        .join()
        .unwrap();

        received.recv_timeout(Duration::from_secs(5)).unwrap().join().unwrap();
        runtime.stop().unwrap();
    }
}
