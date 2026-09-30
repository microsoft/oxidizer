// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Poll};

use futures::future::{FutureExt, LocalBoxFuture};
use pin_project::pin_project;

use crate::runtime::builder::RuntimeBuilder;
use crate::runtime::context::Builtins;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::error::Error;
use crate::runtime::thread::assert_not_flagged;
use crate::task::JoinError;
use crate::task::scheduler::TaskScheduler;

type BoxedFutureFactory<'a, R> = Box<dyn (FnOnce(Builtins) -> LocalBoxFuture<'a, R>) + 'a + Send>;

struct ScopedJoin(mpsc::Receiver<()>);

impl Drop for ScopedJoin {
    fn drop(&mut self) {
        self.0.recv().expect_err("scoped storage signals destruction by disconnecting");
    }
}

#[pin_project]
struct ScopedStorage<T> {
    #[pin]
    inner: T,
    // Drop order matters: signal only after caller-borrowing storage is destroyed.
    completion: mpsc::Sender<()>,
}

impl<FF> ScopedStorage<FF> {
    fn into_future<F, R>(self, cx: Builtins) -> ScopedStorage<impl Future<Output = R>>
    where
        FF: FnOnce(Builtins) -> F,
        F: Future<Output = R>,
    {
        let Self { inner, completion } = self;
        ScopedStorage {
            inner: async move { inner(cx).await },
            completion,
        }
    }
}

impl<F: Future> Future for ScopedStorage<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

/// Owns an Arty runtime's workers and their shutdown.
///
/// Use this type to run asynchronous work from synchronous code. Construction
/// starts one asynchronous worker per selected processor. A task stays on its
/// worker until it finishes or is cancelled.
///
/// [`run`](Self::run) consumes the runtime and stops it after a root task finishes.
/// [`block_on`](Self::block_on) borrows it, allowing several calls and tasks that
/// borrow caller-owned data. For independently submitted tasks, obtain a
/// [`TaskScheduler`] with [`task_scheduler`](Self::task_scheduler).
///
/// # Shutdown
///
/// Dropping the owner requests shutdown and waits for workers to stop. Pending
/// asynchronous tasks and queued blocking callbacks are cancelled; blocking
/// callbacks already running are allowed to finish. A blocking callback that
/// never returns can therefore prevent shutdown from completing.
///
/// Dropping the owner from one of its own blocking callbacks requests shutdown
/// without waiting for that callback to finish. Scheduler and [`Builtins`]
/// handles do not keep the runtime running after its owner is dropped.
///
/// # Panics
///
/// Dropping the owner on an asynchronous Arty worker panics. Keep ownership on
/// a thread where blocking is allowed.
///
/// # Examples
///
/// Submit work from synchronous code and stop after receiving its result:
///
/// ```
/// use arty::runtime::Runtime;
///
/// let runtime = Runtime::new()?;
/// let task = runtime.task_scheduler().spawn(async |_| 42);
/// assert_eq!(task.wait()?, 42);
/// runtime.stop();
/// runtime.wait();
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Runtime {
    dispatcher: DispatcherClient,
}

impl Runtime {
    /// Creates and starts a runtime with the default configuration.
    ///
    /// Equivalent to [`Runtime::builder().build()`](RuntimeBuilder::build).
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the processor selection cannot be satisfied.
    ///
    /// # Panics
    ///
    /// Panics if worker creation or initialization fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// assert_eq!(runtime.run(async |_| 42)?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new() -> Result<Self, Error> {
        RuntimeBuilder::new().build()
    }

    /// Returns a builder for configuring a runtime before starting its workers.
    ///
    /// See [`RuntimeBuilder`] for the defaults and available settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool_policy(BlockingPoolPolicy::shared(4));
    /// ```
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Returns a detached scheduler that selects workers round-robin.
    ///
    /// Use it to submit work from outside the runtime or distribute independent
    /// tasks across workers. In contrast, [`Builtins::scheduler`] keeps child
    /// tasks on their parent's worker.
    ///
    /// Clones and separately obtained schedulers share the runtime's selection
    /// sequence. Selection does not imply execution or completion order.
    /// Retaining a scheduler does not keep the runtime running.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let scheduler = runtime.task_scheduler();
    /// let first = scheduler.spawn(async |_| 20);
    /// let second = scheduler.spawn(async |_| 22);
    /// assert_eq!(first.wait()? + second.wait()?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    #[inline]
    pub fn task_scheduler(&self) -> TaskScheduler {
        TaskScheduler::detached(self.dispatcher.clone())
    }

    /// Runs a root task and shuts down this runtime.
    ///
    /// Invokes `future_factory` on a worker with its owned [`Builtins`], then
    /// blocks the calling thread until the task finishes. The future stays on
    /// that worker and need not be [`Send`].
    ///
    /// The runtime is dropped after receiving the result, following its
    /// [shutdown rules](Self#shutdown). Await any child tasks that must finish
    /// before the root returns; other pending asynchronous work is cancelled.
    /// Captured values and returned results are not automatically relocated.
    ///
    /// # Errors
    ///
    /// Returns [`JoinError`] if the factory or future panics, or if shutdown
    /// cancels or rejects the root task. A `Result` returned by the task is
    /// preserved inside the outer task result.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous Arty worker.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let answer = runtime.run(async |cx| cx.scheduler().spawn(async |_| 6 * 7).await)??;
    /// assert_eq!(answer, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn run<FF, F, R>(self, future_factory: FF) -> Result<R, JoinError>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        assert_not_flagged();
        self.task_scheduler().spawn(future_factory).wait()
    }

    /// Requests shutdown without blocking the calling thread.
    ///
    /// May be called repeatedly, including from a task. Immediately rejects new
    /// submissions, cancels pending asynchronous and local tasks, and prevents
    /// queued blocking callbacks from starting. Already-running blocking
    /// callbacks cannot be forcibly interrupted.
    ///
    /// Cancelled or rejected joins return [`JoinError`] with `is_shutdown() == true`.
    /// Use [`wait`](Self::wait) from synchronous code to wait for workers and
    /// running blocking callbacks to finish.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let scheduler = runtime.task_scheduler();
    /// runtime.stop();
    /// let error = scheduler
    ///     .spawn(async |_| 42)
    ///     .wait()
    ///     .expect_err("submission follows shutdown");
    /// assert!(error.is_shutdown());
    /// runtime.wait();
    /// # Ok::<(), arty::runtime::Error>(())
    /// ```
    pub fn stop(&self) {
        self.dispatcher.stop();
    }

    /// Blocks until this runtime's workers have stopped.
    ///
    /// This does not request shutdown. Call [`stop`](Self::stop) first unless
    /// another task will request it. Repeated calls are allowed, and the method
    /// returns immediately after shutdown has completed.
    ///
    /// The wait includes blocking callbacks that have already started, not
    /// successful completion of cancelled asynchronous tasks. Use task joins to
    /// receive results before requesting shutdown.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous Arty worker.
    /// Also panics when called from a blocking task of this runtime, which cannot
    /// complete while waiting for its own shutdown.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// runtime.stop();
    /// runtime.wait();
    /// # Ok::<(), arty::runtime::Error>(())
    /// ```
    pub fn wait(&self) {
        assert_not_flagged();
        assert!(
            !self.dispatcher.is_current_blocking_task(),
            "a runtime blocking task cannot wait for its own shutdown"
        );
        self.dispatcher.wait();
    }

    /// Runs a borrowing task and blocks until it finishes.
    ///
    /// Unlike [`run`](Self::run), this borrows the runtime and allows
    /// `future_factory` and its future to borrow caller-owned data. The factory
    /// runs on a worker, not on the calling thread. Both the factory and future
    /// are destroyed before the method returns, on success or failure.
    ///
    /// Return owned data or modify borrowed data in place; results cannot borrow
    /// the caller's stack. Captures and results are not automatically relocated.
    /// The runtime remains available for further work until stopped or dropped.
    ///
    /// # Errors
    ///
    /// Returns [`JoinError`] if the factory or future panics, or if shutdown
    /// cancels or rejects the task. Rejection does not invoke the factory.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous Arty worker.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let mut message = String::from("Hello");
    /// runtime.block_on(async |_| message.push_str(", Arty"))?;
    /// assert_eq!(message, "Hello, Arty");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn block_on<'a, FF, F, R>(&self, future_factory: FF) -> Result<R, JoinError>
    where
        FF: FnOnce(Builtins) -> F + Send + 'a,
        F: Future<Output = R> + 'a,
        R: Send + 'static,
    {
        assert_not_flagged();
        let (completion, destroyed) = mpsc::channel();
        let _join = ScopedJoin(destroyed);
        let storage = ScopedStorage {
            inner: future_factory,
            completion,
        };
        let factory: BoxedFutureFactory<'a, R> = Box::new(move |cx| storage.into_future(cx).boxed_local());
        // SAFETY: ScopedJoin cannot return or unwind until the borrowing factory/future
        // is destroyed. The final sender is dropped after those fields, including on
        // cancellation or panic. Receiving the result alone is not a destruction guarantee.
        let factory = unsafe { std::mem::transmute::<BoxedFutureFactory<'a, R>, BoxedFutureFactory<'static, R>>(factory) };
        self.dispatcher.spawn(factory).wait()
    }

    pub(in crate::runtime) const fn with_dispatcher(dispatcher: DispatcherClient) -> Self {
        Self { dispatcher }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
        if !self.dispatcher.is_current_blocking_task() {
            self.wait();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::cell::RefCell;
    use std::future::ready;
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::rc::Rc;
    use std::sync::mpsc::TryRecvError;
    use std::sync::{Arc, Mutex};
    use std::task::Waker;
    use std::thread::{self, ThreadId};

    use testing_aids::{TEST_TIMEOUT, execute_or_terminate_process};

    use super::*;

    struct DropAction<F: FnMut()>(F);

    impl<F: FnMut()> Drop for DropAction<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }

    fn check_storage_drop_order(panic_in_drop: bool) {
        let (completion, destroyed) = mpsc::channel();
        let observations = RefCell::new(Vec::new());
        let record = || observations.borrow_mut().push(destroyed.try_recv());
        let storage = ScopedStorage {
            inner: (
                DropAction(|| {
                    record();
                    if panic_in_drop {
                        panic_any(17u32);
                    }
                }),
                DropAction(record),
            ),
            completion,
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| drop(storage)));
        assert_eq!(
            (outcome.is_err(), observations.into_inner(), destroyed.try_recv()),
            (
                panic_in_drop,
                vec![Err(TryRecvError::Empty), Err(TryRecvError::Empty)],
                Err(TryRecvError::Disconnected),
            ),
        );
    }

    #[test]
    fn scoped_storage_signals_after_all_borrowed_fields_drop() {
        check_storage_drop_order(false);
    }

    #[test]
    fn scoped_storage_signals_after_panicking_destructor_fields_drop() {
        check_storage_drop_order(true);
    }

    #[test]
    fn queued_scoped_factory_disconnects_after_its_borrowed_capture_drops() {
        let (completion, destroyed) = mpsc::channel();
        let mut observed = None;
        let capture = DropAction(|| observed = Some(destroyed.try_recv()));
        let storage = ScopedStorage {
            inner: move |_: Builtins| {
                drop(capture);
                ready(())
            },
            completion,
        };
        let queued = move |cx| storage.into_future(cx);
        drop(queued);
        assert_eq!(
            (observed, destroyed.try_recv()),
            (Some(Err(TryRecvError::Empty)), Err(TryRecvError::Disconnected)),
        );
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct DropRecord {
        text: String,
        thread: Option<ThreadId>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SendOnlyResult;

    struct BorrowingFuture<'a> {
        record: &'a mut DropRecord,
        panic: Option<Arc<()>>,
        _local: Rc<()>,
    }

    impl Future for BorrowingFuture<'_> {
        type Output = SendOnlyResult;

        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            if let Some(payload) = self.panic.take() {
                panic_any(payload);
            }
            Poll::Ready(SendOnlyResult)
        }
    }

    impl Drop for BorrowingFuture<'_> {
        fn drop(&mut self) {
            self.record.text.push_str("dropped");
            self.record.thread = Some(thread::current().id());
        }
    }

    #[test]
    fn ready_scoped_future_keeps_completion_connected_until_destruction() {
        let (completion, destroyed) = mpsc::channel();
        let mut record = DropRecord::default();
        let mut storage = Box::pin(ScopedStorage {
            inner: BorrowingFuture {
                record: &mut record,
                panic: None,
                _local: Rc::new(()),
            },
            completion,
        });
        let outcome = storage.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert_eq!(
            (outcome, destroyed.try_recv()),
            (Poll::Ready(SendOnlyResult), Err(TryRecvError::Empty))
        );
        drop(storage);
        assert_eq!(
            (record, destroyed.try_recv()),
            (
                DropRecord {
                    text: "dropped".to_owned(),
                    thread: Some(thread::current().id())
                },
                Err(TryRecvError::Disconnected),
            ),
        );
    }

    #[test]
    fn scoped_join_waits_for_storage_destruction_during_caller_unwind() {
        execute_or_terminate_process(|| {
            let (completion, destroyed) = mpsc::channel();
            let record = Mutex::new(None);
            let payload = Arc::new(());
            let expected = Arc::clone(&payload);
            thread::scope(|scope| {
                let (unwinding, unwind_started) = mpsc::channel();
                let storage = ScopedStorage {
                    inner: DropAction(|| *record.lock().unwrap() = Some(thread::current().id())),
                    completion,
                };
                let worker = scope.spawn(move || {
                    unwind_started.recv_timeout(TEST_TIMEOUT).unwrap();
                    drop(storage);
                });
                let worker_id = worker.thread().id();
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let _join = ScopedJoin(destroyed);
                    let _unwind = DropAction(|| unwinding.send(()).unwrap());
                    panic_any(payload);
                }));
                let actual = outcome.unwrap_err().downcast::<Arc<()>>().unwrap();
                assert_eq!((Arc::ptr_eq(&actual, &expected), *record.lock().unwrap()), (true, Some(worker_id)));
                worker.join().unwrap();
            });
        });
    }

    #[cfg(not(miri))]
    fn check_borrowed_future_completion(panics: bool) {
        execute_or_terminate_process(|| {
            let runtime = Runtime::builder()
                .processor_count(crate::runtime::ProcessorCount::exactly(1))
                .build()
                .unwrap();
            let worker = runtime.block_on(async |_| thread::current().id()).unwrap();
            let expected = Arc::new(());
            let payload = panics.then(|| Arc::clone(&expected));
            let mut record = DropRecord::default();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                runtime.block_on(async |_| {
                    BorrowingFuture {
                        record: &mut record,
                        panic: payload,
                        _local: Rc::new(()),
                    }
                    .await
                })
            }))
            .unwrap();
            match outcome {
                Ok(value) => {
                    assert!(!panics);
                    assert_eq!(value, SendOnlyResult);
                }
                Err(error) => {
                    assert!(panics);
                    assert!(error.is_panic());
                    #[cfg(feature = "macros")]
                    {
                        let payload = catch_unwind(AssertUnwindSafe(|| -> () { error.resume() })).unwrap_err();
                        assert!(Arc::ptr_eq(&payload.downcast::<Arc<()>>().unwrap(), &expected));
                    }
                }
            }
            assert_eq!(
                (record, runtime.block_on(async |_| 23).unwrap()),
                (
                    DropRecord {
                        text: "dropped".to_owned(),
                        thread: Some(worker)
                    },
                    23
                ),
            );
        });
    }

    #[cfg(not(miri))]
    #[test]
    fn scoped_return_waits_for_borrowed_non_send_future_destructor() {
        check_borrowed_future_completion(false);
    }

    #[cfg(not(miri))]
    #[test]
    fn scoped_poll_panic_is_joined_after_borrowed_future_destruction() {
        check_borrowed_future_completion(true);
    }
}
