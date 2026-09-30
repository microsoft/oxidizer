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
/// There is one asynchronous worker per selected processor. Each task stays on
/// its initial worker until completion or cancellation. Use [`task_scheduler`](Self::task_scheduler)
/// to obtain a cheap, cloneable submission handle; retaining that handle does not
/// prevent the runtime from stopping when this owner is dropped.
///
/// Dropping the runtime stops it and blocks until shutdown completes. Do not drop
/// the owner on an asynchronous runtime worker. Methods that block also reject
/// calls from asynchronous runtime workers.
/// Dropping the owner from one of its blocking tasks requests shutdown without
/// waiting for that task to join itself.
///
/// This type is [`Send`] and [`Sync`], but is not a relocatable [`ThreadAware`](thread_aware::ThreadAware)
/// capability. Task capabilities are available through [`Builtins`].
///
/// See the [documentation guides](crate#documentation) for entry-point
/// choices, cancellation, and the distinction between a task result and shutdown.
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
    pub fn new() -> Result<Self, Error> {
        RuntimeBuilder::new().build()
    }

    /// Starts configuring a runtime before its workers are created.
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Returns a detached scheduler that selects workers round-robin.
    ///
    /// Clones and separately obtained schedulers share the runtime's selection
    /// sequence. Selection does not imply execution or completion order.
    #[must_use]
    #[inline]
    pub fn task_scheduler(&self) -> TaskScheduler {
        TaskScheduler::detached(self.dispatcher.clone())
    }

    /// Runs a root task, waits for its result, then shuts down the runtime.
    ///
    /// The callback receives owned [`Builtins`] on a worker, not the calling thread.
    /// Await any asynchronous children that must finish before the root returns;
    /// returning from the root does not drain other asynchronous tasks.
    ///
    /// Captures and results are not automatically relocated.
    ///
    /// # Errors
    ///
    /// Returns [`JoinError`] if the root task panics or shutdown cancels it.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous runtime worker.
    pub fn run<FF, F, R>(self, future_factory: FF) -> Result<R, JoinError>
    where
        FF: FnOnce(Builtins) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        assert_not_flagged();
        self.task_scheduler().spawn(future_factory).wait()
    }

    #[doc = include_str!("../../docs/snippets/fn_runtime_stop.md")]
    pub fn stop(&self) {
        self.dispatcher.stop();
    }

    /// Waits for shutdown to finish. May be called more than once.
    ///
    /// This does not request shutdown; call [`stop`](Self::stop) first unless
    /// another task will request it. Cancelled joins report shutdown errors.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous runtime worker.
    /// Also panics when called from a blocking task of this runtime, which cannot
    /// complete while waiting for its own shutdown.
    pub fn wait(&self) {
        assert_not_flagged();
        assert!(
            !self.dispatcher.is_current_blocking_task(),
            "a runtime blocking task cannot wait for its own shutdown"
        );
        self.dispatcher.wait();
    }

    /// Runs a task that may borrow the caller's stack, blocking until it completes.
    ///
    /// The borrowing factory and future are destroyed before this method returns
    /// either success or failure. Captures and results are not automatically relocated.
    /// Shutdown cancels pending work and rejects new work without invoking its factory.
    ///
    /// Results must be owned, `Send`, and `'static`; returning a reference into the
    /// caller's stack is not supported. Mutate borrowed caller-owned data or return
    /// an owned result instead.
    ///
    /// ```
    /// let runtime = arty::runtime::Runtime::new().unwrap();
    /// let mut message = String::from("Hello");
    /// runtime
    ///     .block_on(async |_| message.push_str(", Arty"))
    ///     .expect("the runtime remains running until this task completes");
    /// assert_eq!(message, "Hello, Arty");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`JoinError`] if the task panics or shutdown cancels/rejects it.
    ///
    /// # Panics
    ///
    /// Panics if called from an asynchronous runtime worker.
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
                .processor_count(crate::runtime::ProcessorCount::exactly(std::num::NonZeroUsize::MIN))
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
