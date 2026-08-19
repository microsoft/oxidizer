// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::cell::UnsafeCell;
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task;
#[cfg(debug_assertions)]
use std::{backtrace::Backtrace, sync::Arc};

use events_once::RawLocalPooledSender;

use crate::WakeSignal;
#[cfg(debug_assertions)]
use crate::{DiagnosticWaker, DiagnosticWakerRegistry};

/// A task registered with the async task executor, in its fully typed form.
///
/// This facilitates the executor's internal bookkeeping and state management around a task.
/// The executor itself typically handles the task in obscured form, as a [`TypeErasedTask`],
/// as it does not care about the specific type information - only the task itself does.
#[derive(Debug)]
pub(crate) struct Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    /// `None` if the task has either completed or has been abandoned by the executor (e.g. because
    /// it is shutting down).
    ///
    /// We use `UnsafeCell` to allow the `Task` to be accessed from multiple viewpoints, with the
    /// executor using both the payload and the wake signal, and the wakers emitted by the
    /// wake signal using the wake signal. All access is via shared references to ensure no
    /// aliasing violations, with `UnsafeCell` used when mutation of the contents is needed.
    /// (Outer) mutation of the fields only occurs by the executor, ensuring thread-safety.
    payload: UnsafeCell<Option<Payload<F, R>>>,

    /// `None` if the task has not been initialized yet.
    ///
    /// We use `UnsafeCell` to allow the `Task` to be accessed from multiple viewpoints, with the
    /// executor using both the payload and the wake signal, and the wakers emitted by the
    /// wake signal using the wake signal. All access is via shared references to ensure no
    /// aliasing violations, with `UnsafeCell` used when mutation of the contents is needed.
    /// (Outer) mutation of the fields only occurs by the executor, ensuring thread-safety.
    wake_signal: UnsafeCell<Option<WakeSignal>>,

    /// In debug builds, we store backtraces of waker clones here, to help detect waker leaks.
    #[cfg(debug_assertions)]
    diagnostic_waker_registry: Arc<DiagnosticWakerRegistry>,

    // Technically, the wakers also touch the wake signal from other threads but that is
    // private business of the wake signal and nothing that is visible to this type.
    _single_threaded: PhantomData<*const ()>,

    _requires_pin: PhantomPinned,
}

impl<F, R> Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    #[must_use]
    pub(crate) fn new(future: F, result_tx: RawLocalPooledSender<R>) -> Self {
        Self {
            payload: UnsafeCell::new(Some(Payload { future, result_tx })),
            wake_signal: UnsafeCell::new(None),
            #[cfg(debug_assertions)]
            diagnostic_waker_registry: Arc::new(DiagnosticWakerRegistry::new()),
            _single_threaded: PhantomData,
            _requires_pin: PhantomPinned,
        }
    }
}

impl<F, R> TypeErasedTask for Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    fn poll(self: Pin<&Self>) -> task::Poll<()> {
        // SAFETY: The `Task` is single-threaded and we only ever create temporary references
        // to `payload` that do not escape `Task` methods, so we know there cannot be a conflicting
        // reference to this field.
        let maybe_payload = unsafe { self.payload.get().as_mut().expect("UnsafeCell pointer cannot be null") };

        let Some(payload) = maybe_payload else {
            unreachable!("attempted to poll a task whose future was already dropped");
        };

        // SAFETY: Our future and the task that owns it are always pinned by design, the
        // `Pin` annotation just gets lost in the type layering. We add it back here.
        let future_as_mut_pinned = unsafe { Pin::new_unchecked(&mut payload.future) };

        // SAFETY: The `Task` is single-threaded and after initialization, we only ever create
        // shared references to the wake signal, so creating a shared reference here is valid.
        // Before initialization, we do not create any escaping references to the wake signal.
        let wake_signal = unsafe {
            self.wake_signal
                .get()
                .as_ref()
                .expect("UnsafeCell pointer cannot be null")
                .as_ref()
                .expect("task must be initialized before poll()")
        };

        // SAFETY: It is pinned together with the `Task` that contains it, the `Pin` wrapper just
        // goes lost in the whole `UnsafeCell` and `Option` layering.
        let wake_signal = unsafe { Pin::new_unchecked(wake_signal) };

        // SAFETY: After this, we are required to not drop the waker until `.is_inert()` is true.
        // We enforce this via an equivalent safety requirement on the `Task::initialize()`.
        let waker = unsafe { wake_signal.waker() };

        // In debug builds, we wrap the waker with a diagnostic layer, as waker leaks are very
        // damaging due to blocking shutdown and we want to offer maximal debugging information.
        #[cfg(debug_assertions)]
        let waker = DiagnosticWaker::with_inner_and_registry(waker, Arc::clone(&self.diagnostic_waker_registry));

        let mut cx = task::Context::from_waker(&waker);

        // The future we are polling is user code outside our control. It may panic! The executor
        // does not support recovering from such a panic - we terminate the process if that happens.
        // However, the executor is only one layer of runtime logic - in fact, we expect that
        // some higher layer (the task scheduler) will wrap the user code with a panic-handler
        // so we will never actually encounter a panic on this level. The panic trap here is simply
        // a last-chance handler to terminate the application instead of allowing a safety
        // violation to take place - if higher layers do their job, this panic trap will never
        // activate.
        //
        // We `AssertUnwindSafe` here because as we are terminating the process, there is no
        // validity violation that can occur no matter what the type of the future we are dealing
        // with.
        let poll_result = match catch_unwind(AssertUnwindSafe(|| future_as_mut_pinned.poll(&mut cx))) {
            Ok(x) => x,
            Err(panic) => on_unhandled_task_panic(panic),
        };

        match poll_result {
            task::Poll::Ready(result) => {
                let payload = maybe_payload.take().expect("we already validated above that there is a payload");

                // Dropping the completed future executes user code and therefore needs the same
                // panic containment as polling it.
                if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
                    let Payload { future, result_tx } = payload;
                    drop(future);
                    result_tx.send(result);
                })) {
                    on_unhandled_task_panic(panic);
                }

                task::Poll::Ready(())
            }
            task::Poll::Pending => task::Poll::Pending,
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutation causes infinite loops as executor will never shut down.
    fn is_inert(&self) -> bool {
        // SAFETY: The `Task` is single-threaded and after initialization, we only ever create
        // shared references to the wake signal, so creating a shared reference here is valid.
        // Before initialization, we do not create any escaping references to the wake signal.
        let wake_signal = unsafe { self.wake_signal.get().as_ref().expect("UnsafeCell pointer cannot be null").as_ref() };

        wake_signal.is_none_or(WakeSignal::is_inert)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn consume_awakened(&self) -> bool {
        // SAFETY: The `Task` is single-threaded and after initialization, we only ever create
        // shared references to the wake signal, so creating a shared reference here is valid.
        // Before initialization, we do not create any escaping references to the wake signal.
        unsafe {
            self.wake_signal
                .get()
                .as_ref()
                .expect("UnsafeCell pointer cannot be null")
                .as_ref()
                .expect("task must be initialized before consume_awakened()")
                .consume_awakened()
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutation causes resources not to be released, leading to executor shutdown never happening.
    fn abort(self: Pin<&Self>) {
        // SAFETY: The `Task` is single-threaded and we only ever create temporary references
        // to `payload` that do not escape `Task` methods, so we know there cannot be a conflicting
        // reference to this field.
        let payload = unsafe { self.payload.get().as_mut() }.expect("UnsafeCell pointer cannot be null");

        let payload = payload.take();
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
            on_unhandled_task_panic(panic);
        }
    }

    unsafe fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        // SAFETY: The `Task` is single-threaded and before initialization, we do not create any
        // escaping references to the wake signal, so we have exclusive access here. We rely on
        // the caller's safety guarantees that this is not called more than once.
        let maybe_wake_signal = unsafe { self.wake_signal.get().as_mut().expect("UnsafeCell pointer cannot be null") };

        debug_assert!(maybe_wake_signal.is_none());

        *maybe_wake_signal = Some(wake_signal);
    }

    #[cfg(debug_assertions)]
    fn inspect_waker_backtraces(&self, f: &mut dyn FnMut(&Backtrace)) {
        self.diagnostic_waker_registry.inspect_backtraces(f);
    }
}

#[cfg_attr(
    not(test),
    expect(clippy::needless_pass_by_value, reason = "semantically correct to consume the panic")
)]
#[cfg_attr(test, mutants::skip)] // Impractical to unit test process termination.
#[cfg_attr(coverage_nightly, coverage(off))]
fn on_unhandled_task_panic(panic: Box<dyn Any + Send + 'static>) -> ! {
    #[cfg(test)]
    std::panic::resume_unwind(panic);

    #[cfg(not(test))]
    if let Some(s) = panic.downcast_ref::<&str>() {
        eprintln!("unhandled panic in async task - terminating process: {s}");
    } else {
        eprintln!("unhandled panic in async task - terminating process");
    }

    // This should never be reached if the higher layers of Arty runtime correctly wrap
    // every task in a panic-handler designed to forward the panic to the awaiter.
    #[cfg(not(test))]
    std::process::abort();
}

/// Deals with the payload elements of a task - the future and the result.
struct Payload<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    /// The future that will be polled by the executor to progress the task.
    future: F,

    /// The result sender/receiver use pooled event storage provided by the executor, which allows
    /// them to be allocation-free while still being safe by virtue of the executor's shutdown
    /// process, which will not complete until all join handles have been dropped.
    result_tx: RawLocalPooledSender<R>,
}

pub(crate) trait TypeErasedTask {
    /// Polls the task's future, enabling it to make progress.
    ///
    /// There is no result type here - the expectation is that the task itself will deliver any
    /// result out of band, with this poll existing merely to trigger progress.
    ///
    /// It is not valid to poll a task again once it has completed - the internal state is
    /// dropped to a minimal set after completion and polling is no longer valid.
    ///
    /// # Panics
    ///
    /// Panics if the task has not been initialized.
    ///
    /// Panics if the task has already been completed.
    fn poll(self: Pin<&Self>) -> task::Poll<()>;

    /// Whether it is safe to drop the task.
    ///
    /// This indicates whether all resources owned by the task have been released, as the task
    /// may be the owner of memory referenced by other entities in the process, so cannot be
    /// dropped while such resources are still in use by anyone.
    ///
    /// When `abort()` has been called or `poll()` has indicated task completion, there is nothing
    /// else the owner of the task can do to bring this to a value of `true` - we rely on external
    /// parties related to this task (e.g. being awaited by it) to release their resources on their
    /// own initiative when they see the task is no longer executing (e.g. via a dropped future).
    fn is_inert(&self) -> bool;

    /// Swaps the task's "is awakened" flag to false and returns its previous value.
    ///
    /// # Panics
    ///
    /// Panics if the task has not been initialized.
    fn consume_awakened(&self) -> bool;

    /// Clears the inner state of the task, entering a form where no further polling is possible
    /// and any future-specific state is dropped.
    ///
    /// The resources owned by the task may still remain in use - this has no implications with
    /// regard to what `is_inert()` is expected to return.
    fn abort(self: Pin<&Self>);

    /// Initializes the task, providing it the wake signal that it needs to enable polling
    ///
    /// # Safety
    ///
    /// Must not be called more than once.
    ///
    /// Once initialized, the task must not be dropped until `is_inert()` signals it is safe
    /// to do so.
    unsafe fn initialize(self: Pin<&Self>, wake_signal: WakeSignal);

    /// Uses a closure to inspect the backtrace of every waker that is still alive,
    /// to help detect and diagnose waker leaks.
    #[cfg(debug_assertions)]
    fn inspect_waker_backtraces(&self, f: &mut dyn FnMut(&Backtrace));
}

#[cfg(test)]
mod tests {
    use std::future::Ready;
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::Waker;

    use events_once::{Disconnected, RawLocalEventPool};
    use static_assertions::assert_not_impl_any;
    use testing_aids::assert_panic;

    use super::*;
    use crate::testing::TestSubjectFuture;

    struct PanicOnDropFuture(Ready<u64>);

    impl Future for PanicOnDropFuture {
        type Output = u64;

        fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
            Pin::new(&mut self.0).poll(cx)
        }
    }

    impl Drop for PanicOnDropFuture {
        fn drop(&mut self) {
            panic!("panic from future destructor");
        }
    }

    #[test]
    fn smoke_test() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, mut rx) = unsafe { event_pool.as_ref().rent() };

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        // The API contract does not guarantee that the task is inert after initialization,
        // but we know it is because inertness is only invalidated by cloning more wakers.
        // This may change in future versions, though, so be ready to update this test if so.
        assert!(task.is_inert());

        // The future is a trivial one that completes on the first poll.
        let task_result = task.poll();

        assert!(matches!(task_result, task::Poll::Ready(())));

        // The result should now be available via the result channel.
        let mut cx = task::Context::from_waker(Waker::noop());
        let real_result = Pin::new(&mut rx).poll(&mut cx);

        assert!(matches!(real_result, task::Poll::Ready(Ok(42))));

        assert!(task.is_inert());
    }

    #[test]
    fn uninitialized_poll_panics() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, _rx) = unsafe { event_pool.as_ref().rent() };

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        assert_panic!(task.poll());
    }

    #[test]
    #[cfg(debug_assertions)]
    fn inspect_wakers_returns_stashed_wakers() {
        use std::cell::RefCell;
        use std::future::poll_fn;

        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, _rx) = unsafe { event_pool.as_ref().rent() };

        let stashed_wakers = Rc::new(RefCell::new(Vec::new()));

        // Our task just stashes a clone of the waker on every poll.
        let task = pin!(Task::new(
            {
                let stashed_wakers = Rc::clone(&stashed_wakers);

                poll_fn(move |cx| {
                    stashed_wakers.borrow_mut().push(cx.waker().clone());
                    task::Poll::Pending
                })
            },
            tx,
        ));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        // Each poll will stash a waker.
        _ = task.poll();
        _ = task.poll();
        _ = task.poll();

        // The fact that wakers are active implies non-inertness.
        assert!(!task.is_inert());

        let mut inspected_waker_count: usize = 0;

        task.inspect_waker_backtraces(&mut |_| inspected_waker_count += 1);

        assert_eq!(inspected_waker_count, stashed_wakers.borrow().len());

        // Drop the task state and the stash to make the task inert, as required to drop the task.
        task.abort();
        drop(stashed_wakers);

        assert!(task.is_inert());
    }

    #[test]
    fn future_and_event_dropped_after_abort() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, mut rx) = unsafe { event_pool.as_ref().rent() };

        let shared_state = Rc::new(123);
        let shared_state_weak = Rc::downgrade(&shared_state);

        let task = pin!(Task::new(async move { *shared_state }, tx,));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        task.abort();

        // The state held by the task must have been dropped.
        assert!(shared_state_weak.upgrade().is_none());

        // The event receiver must now be disconnected.
        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn future_dropped_after_poll_complete() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, _rx) = unsafe { event_pool.as_ref().rent() };

        let shared_state = Rc::new(123);
        let shared_state_weak = Rc::downgrade(&shared_state);

        let task = pin!(Task::new(async move { *shared_state }, tx,));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        let result = task.poll();
        assert!(matches!(result, task::Poll::Ready(())));

        // The state held by the task must have been dropped.
        assert!(shared_state_weak.upgrade().is_none());
    }

    #[test]
    fn panic_while_dropping_completed_future_is_contained() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, mut rx) = unsafe { event_pool.as_ref().rent() };

        let task = pin!(Task::new(PanicOnDropFuture(std::future::ready(42)), tx));
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        assert_panic!(task.poll());

        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn panic_while_dropping_aborted_future_is_contained() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, mut rx) = unsafe { event_pool.as_ref().rent() };

        let task = pin!(Task::new(PanicOnDropFuture(std::future::ready(42)), tx));
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        assert_panic!(task.abort());

        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn poll_after_complete_panics() {
        let event_pool = pin!(RawLocalEventPool::<u64>::new());

        // SAFETY: We are required to drop this before the pool - we do.
        let (tx, _rx) = unsafe { event_pool.as_ref().rent() };

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        // SAFETY: We organize the test so the task is inert and safe to drop by end of test.
        unsafe {
            task.initialize(WakeSignal::fake());
        }

        // The future is a trivial one that completes on the first poll.
        let task_result = task.poll();

        assert!(matches!(task_result, task::Poll::Ready(())));

        // A poll after completion is not legal.
        assert_panic!(task.poll());
    }

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(Task<TestSubjectFuture, ()>: Send, Sync);
    }
}
