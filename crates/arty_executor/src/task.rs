// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
#[cfg(all(debug_assertions, test))]
use std::backtrace::Backtrace;
use std::cell::{Cell, UnsafeCell};
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
#[cfg(all(debug_assertions, test))]
use std::sync::Arc;
use std::task;

use events_once::PooledLocalSender;

#[cfg(all(debug_assertions, test))]
use crate::{DiagnosticWaker, DiagnosticWakerRegistry};
use crate::{TaskOptions, TaskOutcome, WakeSignal};

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

    options: TaskOptions<R>,

    poll_count: Cell<u64>,

    /// Unit tests store backtraces of waker clones here to exercise leak diagnostics.
    #[cfg(all(debug_assertions, test))]
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
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new(future: F, result_tx: PooledLocalSender<R>) -> Self {
        Self::new_with_options(future, result_tx, TaskOptions::default())
    }

    #[must_use]
    pub(crate) fn new_with_options(future: F, result_tx: PooledLocalSender<R>, options: TaskOptions<R>) -> Self {
        Self {
            payload: UnsafeCell::new(Some(Payload {
                future,
                result_tx: Some(result_tx),
            })),
            wake_signal: UnsafeCell::new(None),
            options,
            poll_count: Cell::new(0),
            #[cfg(all(debug_assertions, test))]
            diagnostic_waker_registry: Arc::new(DiagnosticWakerRegistry::new()),
            _single_threaded: PhantomData,
            _requires_pin: PhantomPinned,
        }
    }

    #[cfg(test)]
    fn has_payload(&self) -> bool {
        // SAFETY: Tests inspect this only on the task's owner thread.
        unsafe { self.payload.get().as_ref() }
            .expect("UnsafeCell pointer cannot be null")
            .is_some()
    }
}

impl<F, R> TypeErasedTask for Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    fn poll(self: Pin<&Self>) -> task::Poll<()> {
        self.poll_count.set(self.poll_count.get().saturating_add(1));
        self.options.with_context(|| self.poll_in_context())
    }

    fn report_created(&self) {
        self.options.report_created();
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

    fn clear_queued_notification(&self) {
        // SAFETY: The executor calls this only for a live initialized task while draining its
        // wake queue.
        unsafe {
            self.wake_signal
                .get()
                .as_ref()
                .expect("UnsafeCell pointer cannot be null")
                .as_ref()
                .expect("task must be initialized before clearing a wake notification")
                .clear_queued_notification();
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutation causes resources not to be released, leading to executor shutdown never happening.
    fn abort(self: Pin<&Self>) {
        // SAFETY: Initialized wake metadata is retired only by the task's owner.
        let wake_signal = unsafe { &*self.wake_signal.get() };
        if let Some(wake_signal) = wake_signal {
            wake_signal.retire();
        }

        // SAFETY: The `Task` is single-threaded and we only ever create temporary references
        // to `payload` that do not escape `Task` methods, so we know there cannot be a conflicting
        // reference to this field.
        let payload = unsafe { self.payload.get().as_mut() }.expect("UnsafeCell pointer cannot be null");

        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The payload is pinned with Task and only the executor accesses it.
            // Cancellation must drop a polled future in place, just like completion.
            unsafe { Pin::new_unchecked(payload) }.set(None);
        })) {
            on_unhandled_task_panic(panic);
        }
    }

    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        // SAFETY: The `Task` is single-threaded and before initialization, we do not create any
        // escaping references to the wake signal, so we have exclusive access here. We rely on
        // the caller's safety guarantees that this is not called more than once.
        let maybe_wake_signal = unsafe { self.wake_signal.get().as_mut().expect("UnsafeCell pointer cannot be null") };

        assert!(maybe_wake_signal.is_none(), "task must be initialized exactly once");

        // SAFETY: initialization is called once, so the slot contains None and
        // owns no previous signal. Writing avoids generating drop glue for an
        // impossible old Some value on every task registration.
        unsafe { std::ptr::write(maybe_wake_signal, Some(wake_signal)) };
    }

    #[cfg(all(debug_assertions, test))]
    fn inspect_waker_backtraces(&self, f: &mut dyn FnMut(&Backtrace)) {
        self.diagnostic_waker_registry.inspect_backtraces(f);
    }
}

impl<F, R> Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    fn poll_in_context(self: Pin<&Self>) -> task::Poll<()> {
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

        let poll_result = {
            // SAFETY: The borrowed polling waker cannot escape this block. Clones retain the
            // independently pooled state and may outlive both the poll and the task.
            let waker = unsafe { wake_signal.waker_ref() };

            // Unit tests wrap the waker to exercise clone backtrace diagnostics.
            #[cfg(all(debug_assertions, test))]
            let waker = DiagnosticWaker::with_inner_and_registry(waker.clone(), Arc::clone(&self.diagnostic_waker_registry));

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
            match catch_unwind(AssertUnwindSafe(|| future_as_mut_pinned.poll(&mut cx))) {
                Ok(x) => x,
                Err(panic) => {
                    self.options.report_terminal(TaskOutcome::Panicked, self.poll_count.get());
                    on_unhandled_task_panic(panic);
                }
            }
        };

        match poll_result {
            task::Poll::Ready(result) => {
                let outcome = self.options.classify(&result);
                wake_signal.retire();
                let result_tx = payload
                    .result_tx
                    .take()
                    .expect("the result sender is present until this task completes");

                // Dropping the completed future executes user code and therefore needs the same
                // panic containment as polling it.
                if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
                    // SAFETY: The payload is pinned with Task and only the executor accesses it.
                    // Pin::set drops the future in place before clearing the payload.
                    unsafe { Pin::new_unchecked(maybe_payload) }.set(None);
                    result_tx.send(result);
                })) {
                    self.options.report_terminal(TaskOutcome::Panicked, self.poll_count.get());
                    on_unhandled_task_panic(panic);
                }

                if let Some(outcome) = outcome {
                    self.options.report_terminal(outcome, self.poll_count.get());
                }

                task::Poll::Ready(())
            }
            task::Poll::Pending => task::Poll::Pending,
        }
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

    /// The result sender/receiver use pooled event storage whose detached slots are retained by
    /// their endpoints. Join handles may therefore outlive both the task and the executor.
    result_tx: Option<PooledLocalSender<R>>,
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

    /// Emits task-created telemetry after executor registration, if configured.
    fn report_created(&self);

    /// Swaps the task's "is awakened" flag to false and returns its previous value.
    ///
    /// # Panics
    ///
    /// Panics if the task has not been initialized.
    fn consume_awakened(&self) -> bool;

    /// Records that the executor drained this task's queued wake notification.
    fn clear_queued_notification(&self);

    /// Clears the inner state of the task, entering a form where no further polling is possible
    /// and any future-specific state is dropped.
    ///
    /// Independently pooled wakers remain valid but become inert.
    fn abort(self: Pin<&Self>);

    /// Initializes the task, providing it the wake signal that it needs to enable polling
    ///
    /// # Panics
    ///
    /// Panics if called more than once.
    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal);

    /// Uses a closure to inspect the backtrace of every waker that is still alive,
    /// to help detect and diagnose waker leaks.
    #[cfg(all(debug_assertions, test))]
    fn inspect_waker_backtraces(&self, f: &mut dyn FnMut(&Backtrace));
}

#[cfg(test)]
mod tests {
    use std::future::{Ready, pending};
    use std::pin::pin;
    use std::rc::Rc;
    #[cfg(not(debug_assertions))]
    use std::sync::Arc;
    use std::task::Waker;

    use events_once::{Disconnected, LocalEventPool};
    use observed_testing::{TEST_ID, test_emitter};
    use static_assertions::assert_not_impl_any;
    use testing_aids::assert_panic;

    use super::*;
    use crate::testing::TestSubjectFuture;

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        clippy::unnecessary_wraps,
        reason = "TaskOptions classifiers receive outputs by reference and may omit terminal reporting"
    )]
    fn succeeded(_: &u64) -> Option<TaskOutcome> {
        Some(TaskOutcome::Succeeded)
    }

    struct PinnedDropFuture {
        polled_at: Cell<*const Self>,
        dropped: Rc<Cell<bool>>,
        completes: bool,
        _pin: PhantomPinned,
    }

    impl Future for PinnedDropFuture {
        type Output = u64;

        fn poll(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> task::Poll<Self::Output> {
            self.polled_at.set(std::ptr::from_ref(self.as_ref().get_ref()));
            if self.completes {
                task::Poll::Ready(42)
            } else {
                task::Poll::Pending
            }
        }
    }

    impl Drop for PinnedDropFuture {
        fn drop(&mut self) {
            assert_eq!(self.polled_at.get(), std::ptr::from_ref(self));
            self.dropped.set(true);
        }
    }

    fn check_pinned_future_destruction(completes: bool) {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();
        let dropped = Rc::new(Cell::new(false));
        let task = pin!(Task::new(
            PinnedDropFuture {
                polled_at: Cell::new(std::ptr::null()),
                dropped: Rc::clone(&dropped),
                completes,
                _pin: PhantomPinned,
            },
            tx,
        ));
        let task = task.as_ref();
        task.initialize(WakeSignal::fake());

        let result = task.poll();
        if completes {
            assert!(result.is_ready());
        } else {
            assert!(result.is_pending());
            task.abort();
        }
        assert!(dropped.get());
        assert!(!task.has_payload());

        let mut cx = task::Context::from_waker(Waker::noop());
        let result = Pin::new(&mut rx).poll(&mut cx);
        if completes {
            assert_eq!(result, task::Poll::Ready(Ok(42)));
        } else {
            assert_eq!(result, task::Poll::Ready(Err(Disconnected)));
        }
    }

    #[test]
    fn clear_queued_notification_forwards_to_the_wake_signal() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();
        let task = pin!(Task::new(pending::<u64>(), tx));
        let task = task.as_ref();
        // SAFETY: Tests use this only as an opaque identity.
        let fake_task_ref = unsafe { crate::TaskRef::fake() };
        let queue = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::with_capacity(1)));
        let signal = WakeSignal::new_with_parts(
            Arc::clone(&queue),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Waker::noop().clone(),
            fake_task_ref,
        );
        task.initialize(signal);
        // SAFETY: Initialization populated the pinned wake-signal slot.
        let signal = unsafe { &*task.wake_signal.get() }.as_ref().unwrap();
        // SAFETY: The signal remains pinned with the task.
        let signal = unsafe { Pin::new_unchecked(signal) };
        // SAFETY: The pinned signal remains alive for this borrow.
        let waker = unsafe { signal.waker_ref() };
        waker.wake_by_ref();
        assert!(signal.has_queued_notification());

        task.clear_queued_notification();

        assert!(!signal.has_queued_notification());
        drop(waker);
        task.abort();
    }

    #[test]
    fn completed_future_is_destroyed_at_its_pinned_address() {
        check_pinned_future_destruction(true);
    }

    #[test]
    fn cancelled_future_is_destroyed_at_its_pinned_address() {
        check_pinned_future_destruction(false);
    }

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
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(WakeSignal::fake());

        assert!(task.has_payload());

        // The future is a trivial one that completes on the first poll.
        let task_result = task.poll();

        assert!(matches!(task_result, task::Poll::Ready(())));

        // The result should now be available via the result channel.
        let mut cx = task::Context::from_waker(Waker::noop());
        let real_result = Pin::new(&mut rx).poll(&mut cx);

        assert!(matches!(real_result, task::Poll::Ready(Ok(42))));

        assert!(!task.has_payload());
    }

    #[test]
    fn observed_task_reports_aggregate_poll_count_on_completion() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();
        let (sink, processor) = test_emitter(TEST_ID);
        let polls = Rc::new(Cell::new(0));
        let task = pin!(Task::new_with_options(
            std::future::poll_fn({
                let polls = Rc::clone(&polls);
                move |_| {
                    polls.set(polls.get() + 1);
                    if polls.get() == 2 {
                        task::Poll::Ready(42)
                    } else {
                        task::Poll::Pending
                    }
                }
            }),
            tx,
            TaskOptions::observed(sink.clone(), sink.transfer_context(), "any", succeeded),
        ));
        let task = task.as_ref();
        task.initialize(WakeSignal::fake());
        task.report_created();

        assert!(task.poll().is_pending());
        assert!(task.poll().is_ready());

        let events = processor.events();
        let completed = events.iter().find(|event| event.name() == "arty.rt.task.succeeded").unwrap();
        assert_eq!(completed.field_metrics()[0].field_key(), "polls");
        assert!((completed.field_metrics()[0].value() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn uninitialized_poll_panics() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();

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

        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();

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

        task.initialize(WakeSignal::fake());

        // Each poll will stash a waker.
        _ = task.poll();
        _ = task.poll();
        _ = task.poll();

        assert!(task.has_payload());

        let mut inspected_waker_count: usize = 0;

        task.inspect_waker_backtraces(&mut |_| inspected_waker_count += 1);

        assert_eq!(inspected_waker_count, stashed_wakers.borrow().len());

        // Drop the task payload and the diagnostic wakers it retained.
        task.abort();
        drop(stashed_wakers);

        assert!(!task.has_payload());
    }

    #[test]
    fn future_and_event_dropped_after_abort() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let shared_state = Rc::new(123);
        let shared_state_weak = Rc::downgrade(&shared_state);

        let task = pin!(Task::new(async move { *shared_state }, tx,));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(WakeSignal::fake());

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
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();

        let shared_state = Rc::new(123);
        let shared_state_weak = Rc::downgrade(&shared_state);

        let task = pin!(Task::new(async move { *shared_state }, tx,));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(WakeSignal::fake());

        let result = task.poll();
        assert!(matches!(result, task::Poll::Ready(())));

        // The state held by the task must have been dropped.
        assert!(shared_state_weak.upgrade().is_none());
    }

    #[test]
    fn panic_while_dropping_completed_future_is_contained() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let task = pin!(Task::new(PanicOnDropFuture(std::future::ready(42)), tx));
        let task = task.as_ref();

        task.initialize(WakeSignal::fake());

        assert_panic!(task.poll());

        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn panic_while_dropping_aborted_future_is_contained() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let task = pin!(Task::new(PanicOnDropFuture(std::future::ready(42)), tx));
        let task = task.as_ref();

        task.initialize(WakeSignal::fake());

        assert_panic!(task.abort());

        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn poll_after_complete_panics() {
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(WakeSignal::fake());

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
