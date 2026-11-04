// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

use futures::future::{FutureExt, LocalBoxFuture};
use performables::sync::channel;
use pin_project::pin_project;

use crate::core::ThreadAware;
use crate::runtime::Error;
use crate::runtime::dispatch::DispatcherClient;
use crate::task::{Context, JoinHandle};

type BoxedFutureFactory<'a, R> = Box<dyn (FnOnce(Context) -> LocalBoxFuture<'a, R>) + 'a + Send>;

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
    fn into_future<F, R>(self, cx: Context) -> ScopedStorage<impl Future<Output = R>>
    where
        FF: FnOnce(Context) -> F,
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

    fn poll(self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

/// Submits tasks across an Arty runtime's workers.
///
/// Borrow this scheduler through
/// [`Runtime::scheduler`](crate::runtime::Runtime::scheduler).
/// You cannot clone it or keep it after the `Runtime` value is dropped.
/// To keep child tasks on their parent's worker, use [`Context::scheduler`] instead.
///
/// The runtime chooses where new tasks start.
/// Tasks do not necessarily start or finish in the order you submit them.
/// Await required task results before stopping the runtime.
#[derive(Debug)]
pub struct RuntimeScheduler {
    pub(crate) dispatcher: DispatcherClient,
}

impl RuntimeScheduler {
    pub(crate) const fn new(dispatcher: DispatcherClient) -> Self {
        Self { dispatcher }
    }

    /// Runs a task that borrows the caller's data and waits for it to finish.
    ///
    /// A task factory is a function that creates a future.
    /// The factory runs on a worker, not on the calling thread. Both the
    /// factory and future are dropped before this method returns, even if
    /// the task fails or is cancelled.
    ///
    /// The factory must implement `Send`. The future can borrow the caller's
    /// data and does not need to implement `Send`. The result must implement
    /// `Send + 'static`.
    ///
    /// Return owned data or modify borrowed data in place. Results cannot borrow
    /// the caller's stack. Arty does not automatically update captured values
    /// or results for a different worker.
    /// The runtime can run more tasks until it is stopped or dropped.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the factory or future panics, or if shutdown
    /// cancels or rejects the task.
    /// Its underlying error is a [`JoinError`](crate::task::JoinError).
    /// If Arty rejects the task, it does not call the factory.
    ///
    /// Calling this from any Arty worker returns an error. This includes task
    /// workers, blocking workers, and workers that belong to another runtime.
    /// It also returns an error inside an active `futures` executor. The method
    /// rejects these calls before it submits the task.
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
        FF: FnOnce(Context) -> F + Send + 'a,
        F: Future<Output = R> + 'a,
        R: Send + 'static,
    {
        if crate::runtime::worker::WorkerMarker::is_current() {
            return Err(Error::block_on_from_worker());
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
        futures::executor::block_on(self.dispatcher.spawn(factory)).map_err(Error::new)
    }

    /// Lets the runtime choose a worker for an async task and relocates its data.
    ///
    /// Relocation updates a value's state for a selected worker.
    /// Before starting the task, Arty relocates `data` to the chosen worker.
    /// The source worker is unknown because this scheduler can be used from
    /// outside the runtime. The function pointer receives [`Context`] and
    /// the relocated data. Its future need not be [`Send`], but its result must
    /// be `Send`. The result is not relocated and need not implement `ThreadAware`.
    /// The data, future, and result must be `'static`.
    ///
    /// Use `()` for tasks without input data. For data that does not implement
    /// `ThreadAware`, see [`Unaware`](thread_aware::Unaware) and its guidance
    /// before wrapping it.
    ///
    /// Shutdown rejects tasks without relocating data or calling the factory.
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
    /// assert_eq!(futures::executor::block_on(task)?, 4);
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
    /// let task = runtime
    ///     .scheduler()
    ///     .spawn_anywhere((), |_, ()| async { SendOnly(42) });
    /// let result = futures::executor::block_on(task)?;
    /// assert_eq!(result.0, 42);
    /// runtime.stop()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn spawn_anywhere<D, F, R>(&self, data: D, future_factory: fn(Context, D) -> F) -> JoinHandle<R>
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

    /// Runs a function on a thread from a blocking pool.
    ///
    /// Uses a pool chosen by the runtime for synchronous I/O or other calls
    /// that can stop an async worker from running other tasks.
    /// A panic in this function returns an error through [`JoinHandle`] and
    /// follows the configured [`PanicPolicy`](crate::runtime::PanicPolicy).
    /// The function and its result must implement `Send + 'static`.
    ///
    /// Shutdown rejects new blocking functions and cancels queued ones before they start.
    /// Functions that already run can finish.
    ///
    /// Do not synchronously poll a [`JoinHandle`] for another task submitted to the same pool.
    /// Direct attempts panic. [`block_on`](Self::block_on) also rejects calls
    /// from blocking functions before starting any task.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let task = runtime.scheduler().spawn_blocking(|| 6 * 7);
    /// assert_eq!(futures::executor::block_on(task)?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn spawn_blocking<B, R>(&self, body: B) -> JoinHandle<R>
    where
        B: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
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

    use testing_aids::TEST_TIMEOUT;

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
            inner: move |_: Context| {
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

        fn poll(mut self: Pin<&mut Self>, _: &mut PollContext<'_>) -> Poll<Self::Output> {
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
        let outcome = storage.as_mut().poll(&mut PollContext::from_waker(Waker::noop()));
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
    }

    #[cfg(not(miri))]
    fn check_borrowed_future_completion(panics: bool) {
        let runtime = Runtime::builder()
            .workers(crate::runtime::WorkersPolicy::exactly(1))
            .panic(crate::runtime::PanicPolicy::ignore())
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
                let payload = error.into_panic().expect("the join error was checked as a panic");
                assert!(Arc::ptr_eq(&payload.downcast::<Arc<()>>().unwrap(), &expected));
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
