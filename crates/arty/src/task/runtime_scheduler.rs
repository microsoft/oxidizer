// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::future::{FutureExt, LocalBoxFuture};
use performables::sync::channel;
use pin_project::pin_project;

use crate::core::ThreadAware;
use crate::runtime::Error;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::thread::is_flagged;
use crate::task::{Builtins, JoinHandle};

type BoxedFutureFactory<'a, R> = Box<dyn (FnOnce(Builtins) -> LocalBoxFuture<'a, R>) + 'a + Send>;

struct ScopedJoin(channel::Receiver<()>);

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
    completion: channel::Sender<()>,
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

/// Submits work across an Arty runtime's workers.
///
/// Borrow this scheduler through
/// [`Runtime::scheduler`](crate::runtime::Runtime::scheduler).
/// It cannot be cloned or retained independently of the runtime owner.
/// For worker-affine child tasks, use [`Builtins::scheduler`] instead.
///
/// The runtime chooses where new work starts. Submission order does not imply
/// execution or completion order.
/// Complete required joins before stopping the runtime.
#[derive(Debug)]
pub struct RuntimeScheduler {
    pub(crate) dispatcher: DispatcherClient,
}

impl RuntimeScheduler {
    pub(crate) const fn new(dispatcher: DispatcherClient) -> Self {
        Self { dispatcher }
    }

    /// Reports whether the caller is on one of this runtime's async workers.
    ///
    /// Returns `false` on external threads, blocking-pool threads, and workers
    /// belonging to another runtime.
    #[must_use]
    #[inline]
    pub fn is_on_worker(&self) -> bool {
        self.dispatcher.worker_index(std::thread::current().id()).is_some()
    }

    /// Runs a task that borrows caller-owned data and waits for it to finish.
    ///
    /// The factory runs on a worker, not on the calling thread. Both the
    /// factory and future are destroyed before this method returns, even if
    /// the task fails or is cancelled.
    ///
    /// Return owned data or modify borrowed data in place; results cannot borrow
    /// the caller's stack. Captures and results are not automatically relocated.
    /// The runtime remains available for further work until stopped or dropped.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] with a [`JoinError`](crate::task::JoinError) source
    /// if the factory or future panics, or if shutdown cancels or rejects the task.
    /// Rejection does not invoke the factory.
    ///
    /// Calling this from any async Arty worker returns an error, including
    /// a worker belonging to another runtime. It also returns an error from
    /// inside an already-running `futures` executor, before submitting the task.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let mut message = String::from("Hello");
    /// runtime
    ///     .scheduler()
    ///     .block_on(async |_| message.push_str(", Arty"))?;
    /// assert_eq!(message, "Hello, Arty");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn block_on<'a, FF, F, R>(&self, future_factory: FF) -> Result<R, Error>
    where
        FF: FnOnce(Builtins) -> F + Send + 'a,
        F: Future<Output = R> + 'a,
        R: Send + 'static,
    {
        if is_flagged() {
            return Err(Error::new("block_on cannot be called from an async Arty worker"));
        }
        // Validate the ambient executor before any caller-borrowing work is submitted.
        drop(futures::executor::enter().map_err(Error::new)?);
        let (completion, destroyed) = channel::unbounded();
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
        self.dispatcher.spawn(factory).wait().map_err(Error::new)
    }

    /// Lets the runtime place an async task and relocates its payload.
    ///
    /// Before starting the task, Arty relocates `data` to the chosen worker.
    /// The source worker is unknown because this scheduler can be used from
    /// outside the runtime. The function pointer receives [`Builtins`] and
    /// the relocated data. Its future need not be [`Send`], but its result must
    /// be `Send`. The result is not relocated and need not implement `ThreadAware`.
    ///
    /// Use `()` for work without a payload. For payloads that do not implement
    /// `ThreadAware`, see [`Unaware`](thread_aware::Unaware) and its guidance
    /// before wrapping them.
    ///
    /// Shutdown rejects submissions without relocating data or invoking the factory.
    /// Factory and future panics are reported through the returned [`JoinHandle`].
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let task = runtime
    ///     .scheduler()
    ///     .spawn_anywhere(String::from("Arty"), |_, name| async move { name.len() });
    /// assert_eq!(task.wait()?, 4);
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Results can contain ordinary `Send` values without thread awareness:
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// struct SendOnly(u32);
    /// let runtime = Runtime::new()?;
    /// let result = runtime
    ///     .scheduler()
    ///     .spawn_anywhere((), |_, ()| async { SendOnly(42) })
    ///     .wait()?;
    /// assert_eq!(result.0, 42);
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn spawn_anywhere<D, F, R>(&self, data: D, future_factory: fn(Builtins, D) -> F) -> JoinHandle<R>
    where
        D: ThreadAware + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.dispatcher.spawn(async move |cx| {
            let mut data = data;
            data.relocate(None, cx.thread());
            future_factory(cx, data).await
        })
    }

    /// Submits a synchronous callback to a blocking-task pool.
    ///
    /// Uses a pool chosen by the runtime for synchronous I/O or other work
    /// that would stall an async worker.
    /// The callback's panic is reported through its join.
    ///
    /// Shutdown rejects new callbacks and cancels queued callbacks before they
    /// start. Already-running callbacks are allowed to finish.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let task = runtime.scheduler().spawn_blocking(|| 6 * 7);
    /// assert_eq!(task.wait()?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn spawn_blocking<B, R>(&self, body: B) -> JoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if self.dispatcher.is_shutting_down() {
            return JoinHandle::shutdown();
        }
        self.dispatcher.next_blocking_worker().spawn_blocking(body)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::future::ready;
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::task::Waker;
    use std::thread::{self, ThreadId};

    use testing_aids::{TEST_TIMEOUT, execute_or_terminate_process};

    use super::*;
    use crate::runtime::Runtime;
    use crate::task::JoinError;

    struct DropAction<F: FnMut()>(F);

    impl<F: FnMut()> Drop for DropAction<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }

    #[test]
    #[should_panic(expected = "scoped storage signals destruction by disconnecting")]
    fn scoped_join_rejects_a_completion_message() {
        let (completion, destroyed) = channel::unbounded();
        completion.send(()).unwrap();
        drop(ScopedJoin(destroyed));
    }

    fn check_storage_drop_order(panic_in_drop: bool) {
        let (completion, destroyed) = channel::unbounded();
        let observations = RefCell::new(Vec::new());
        let record = || observations.borrow_mut().push(destroyed.try_recv().unwrap_err().is_empty());
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
            (
                outcome.is_err(),
                observations.into_inner(),
                destroyed.try_recv().unwrap_err().is_closed()
            ),
            (panic_in_drop, vec![true, true], true,),
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
        let (completion, destroyed) = channel::unbounded();
        let mut observed = None;
        let capture = DropAction(|| observed = Some(destroyed.try_recv().unwrap_err().is_empty()));
        let storage = ScopedStorage {
            inner: move |_: Builtins| {
                drop(capture);
                ready(())
            },
            completion,
        };
        let queued = move |cx| storage.into_future(cx);
        drop(queued);
        assert_eq!((observed, destroyed.try_recv().unwrap_err().is_closed()), (Some(true), true),);
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
        let (completion, destroyed) = channel::unbounded();
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
            (outcome, destroyed.try_recv().unwrap_err().is_empty()),
            (Poll::Ready(SendOnlyResult), true)
        );
        drop(storage);
        assert_eq!(
            (record, destroyed.try_recv().unwrap_err().is_closed()),
            (
                DropRecord {
                    text: "dropped".to_owned(),
                    thread: Some(thread::current().id())
                },
                true,
            ),
        );
    }

    #[test]
    fn scoped_join_waits_for_storage_destruction_during_caller_unwind() {
        execute_or_terminate_process(|| {
            let (completion, destroyed) = channel::unbounded();
            let record = Mutex::new(None);
            let payload = Arc::new(());
            let expected = Arc::clone(&payload);
            thread::scope(|scope| {
                let (unwinding, unwind_started) = channel::unbounded();
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
            let worker = runtime.scheduler().block_on(async |_| thread::current().id()).unwrap();
            let expected = Arc::new(());
            let payload = panics.then(|| Arc::clone(&expected));
            let mut record = DropRecord::default();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                runtime.scheduler().block_on(async |_| {
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
                    let error = error.into_source().downcast::<JoinError>().unwrap();
                    assert!(error.is_panic());
                    #[cfg(feature = "macros")]
                    {
                        let payload = catch_unwind(AssertUnwindSafe(|| -> () { error.resume() })).unwrap_err();
                        assert!(Arc::ptr_eq(&payload.downcast::<Arc<()>>().unwrap(), &expected));
                    }
                }
            }
            assert_eq!(
                (record, runtime.scheduler().block_on(async |_| 23).unwrap()),
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
