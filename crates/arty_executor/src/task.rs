// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "inspection")]
use std::any::Any;
use std::cell::{Cell, OnceCell, UnsafeCell};
use std::fmt::Debug;
use std::marker::PhantomPinned;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task;

use observed::Sink;
use pin_project::{pin_project, pinned_drop};
use plurality::{Arc as PoolArc, MultiPool};

#[cfg(test)]
use crate::TaskHandle;
#[cfg(feature = "inspection")]
use crate::TaskState;
use crate::inspection::Inspector;
use crate::{
    ExecutorShared, ShutdownCause, TaskCreatedReporter, TaskHandleSlot, TaskMetadata, TaskPanicPayload, TaskPtr, WakeSignal,
    discard_task_panic_payload,
};

#[derive(Debug, Default)]
pub(crate) struct TaskLinks {
    pub(crate) previous: Cell<Option<TaskPtr>>,
    pub(crate) next: Cell<Option<TaskPtr>>,
    pub(crate) linked: Cell<bool>,
}

/// The typed form of a task that is registered with the executor.
///
/// The task stores its future, completion target, wake state, and bookkeeping.
/// The executor usually accesses it through [`TypeErasedTask`] because only the
/// task needs its concrete types.
#[derive(Debug)]
struct Task<F, R, T, I = ()>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
    /// `None` after the task completes or the executor abandons it during shutdown.
    ///
    /// We use `UnsafeCell` to allow the `Task` to be accessed from multiple viewpoints, with the
    /// executor using both the payload and the wake signal, and the wakers emitted by the
    /// wake signal using the wake signal. All access is through shared references, so there are no
    /// aliasing violations, with `UnsafeCell` used when mutation of the contents is needed.
    /// (Outer) mutation of the fields only occurs by the executor, ensuring thread-safety.
    payload: UnsafeCell<Option<Payload<F, R, T, I>>>,

    /// Initialized once on the owner, before any waker is exposed. The signal is never replaced.
    wake_signal: OnceCell<WakeSignal>,

    handle_slot: TaskHandleSlot,

    metadata: TaskMetadata,

    sink: Sink,

    poll_count: Cell<u64>,
    links: TaskLinks,

    _requires_pin: PhantomPinned,
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "remote owners retain only the allocation; the executor retires non-Send payloads on their owner thread"
)]
// SAFETY: Remote threads can retain and reference-count task cells, but task access is
// private and owner-thread-only. Cross-thread wakers touch only separately synchronized wake
// metadata. Completion and cancellation empty the non-Send payload before the owner releases its
// final scheduling reference, so remote final reclamation cannot drop user state.
// The optional inspector belongs to that payload and is also retired on the owner thread.
// The owner initializes the handle cell before publishing pointers. Remote owners read only
// its immutable template through those pointers, never the cell's initialization state.
unsafe impl<F, R, T, I> Send for Task<F, R, T, I>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
}

// SAFETY: Same invariant as `Send`; shared remote access never reaches task payload fields.
unsafe impl<F, R, T, I> Sync for Task<F, R, T, I>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
}

impl<F, R, T, I> Task<F, R, T, I>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
    fn new(future: F, result_target: T, metadata: TaskMetadata, sink: Sink, inspector: I) -> Self {
        Self {
            payload: UnsafeCell::new(Some(Payload {
                future: FutureStorage::new(future),
                result_target: Some(result_target),
                #[cfg(feature = "inspection")]
                inspector,
                #[cfg(not(feature = "inspection"))]
                _inspector: inspector,
            })),
            wake_signal: OnceCell::new(),
            handle_slot: TaskHandleSlot::new(),
            metadata,
            sink,
            poll_count: Cell::new(0),
            links: TaskLinks::default(),
            _requires_pin: PhantomPinned,
        }
    }
}

pub(crate) fn allocate_task<F, T, I>(
    pool: &MultiPool,
    future: F,
    metadata: TaskMetadata,
    sink: Sink,
    completion: T,
    inspector: I,
) -> Pin<PoolArc<impl TypeErasedTask + 'static>>
where
    F: Future + 'static,
    F::Output: 'static,
    T: TaskCompletion<F::Output>,
    I: Inspector<F>,
{
    pool.alloc_arc_pin_with(|| Task::new(future, completion, metadata, sink, inspector))
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TestCompletion;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl<R: 'static> TaskCompletion<R> for TestCompletion {
    fn completed(self, _result: R) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn allocate_test_task_in(pool: &MultiPool) -> TaskHandle {
    TaskHandle::new(allocate_task(
        pool,
        std::future::pending::<()>(),
        TaskMetadata::default(),
        Sink::noop(),
        TestCompletion,
        (),
    ))
}

impl<F, R, T, I> TypeErasedTask for Task<F, R, T, I>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
    #[cfg(feature = "inspection")]
    fn inspect(&self, state: TaskState, visitor: &mut dyn FnMut(&dyn Any)) {
        // SAFETY: Inspection runs on the owner between cycles or immediately before polling.
        // The shared future reference ends before a poll or retirement can borrow its payload.
        let payload = unsafe { &*self.payload.get() };
        if let Some(payload) = payload
            && let Some(future) = &payload.future.inner
        {
            payload.inspector.inspect(future, state, visitor);
        }
    }

    #[cfg(feature = "inspection")]
    fn is_notified(&self) -> bool {
        self.wake_signal.get().is_some_and(WakeSignal::has_queued_notification)
    }

    fn poll(self: Pin<&Self>, shared: &ExecutorShared) -> task::Poll<()> {
        self.poll_count.set(self.poll_count.get().saturating_add(1));
        self.metadata.with_context(|| self.poll_in_context(shared))
    }

    fn created_reporter(&self) -> Option<TaskCreatedReporter> {
        self.metadata.created_reporter(&self.sink)
    }

    fn links(&self) -> &TaskLinks {
        &self.links
    }

    fn handle_slot(&self) -> &TaskHandleSlot {
        &self.handle_slot
    }

    fn clear_queued_notification(&self) {
        self.wake_signal
            .get()
            .expect("task must be initialized before clearing a wake notification")
            .clear_queued_notification();
    }

    fn promote_remote_notification(&self) -> bool {
        self.wake_signal
            .get()
            .expect("remote notifications retain initialized tasks")
            .promote_remote_notification()
    }

    fn abort(self: Pin<&Self>, cause: ShutdownCause) {
        if let Some(wake_signal) = self.wake_signal.get() {
            wake_signal.retire();
        }

        // SAFETY: The `Task` is single-threaded and we only ever create temporary references
        // to `payload` that do not escape `Task` methods, so we know there cannot be a conflicting
        // reference to this field.
        let maybe_payload = unsafe { self.payload.get().as_mut() }.expect("UnsafeCell pointer cannot be null");

        self.metadata
            .with_context(|| self.finish_terminal(maybe_payload, TerminalOutcome::Cancelled(cause)));
    }

    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        self.wake_signal.set(wake_signal).expect("task must be initialized exactly once");
    }
}

impl<F, R, T, I> Task<F, R, T, I>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
    fn pinned_wake_signal(self: Pin<&Self>) -> Pin<&WakeSignal> {
        let signal = self.get_ref().wake_signal.get().expect("task must be initialized before poll()");
        // SAFETY: OnceCell never removes or replaces the signal, which stays pinned with Task.
        unsafe { Pin::new_unchecked(signal) }
    }

    fn poll_in_context(self: Pin<&Self>, shared: &ExecutorShared) -> task::Poll<()> {
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

        let wake_signal = self.pinned_wake_signal();

        let poll_result = {
            // SAFETY: The borrowed polling waker cannot escape this block. Clones retain the
            // pinned pooled task allocation. Owner retirement empties its payload before a remote
            // clone can release the final reference.
            let waker = unsafe { wake_signal.waker_ref() };

            let mut cx = task::Context::from_waker(&waker);

            match catch_unwind(AssertUnwindSafe(|| future_as_mut_pinned.poll(&mut cx))) {
                Ok(x) => x,
                Err(panic) => return self.finish_panicked(maybe_payload, wake_signal, panic, shared),
            }
        };

        match poll_result {
            task::Poll::Ready(result) => {
                wake_signal.retire();
                self.finish_terminal(maybe_payload, TerminalOutcome::Succeeded(result));
                task::Poll::Ready(())
            }
            task::Poll::Pending => task::Poll::Pending,
        }
    }

    fn finish_panicked(
        self: Pin<&Self>,
        maybe_payload: &mut Option<Payload<F, R, T, I>>,
        wake_signal: Pin<&WakeSignal>,
        panic: TaskPanicPayload,
        shared: &ExecutorShared,
    ) -> task::Poll<()> {
        let payload = maybe_payload.as_mut().expect("a polled task retains its payload");
        let result_target = payload.result_target.take().expect("a polled task retains its result target");

        wake_signal.retire();
        // SAFETY: The future remains pinned with its task and is destroyed in place.
        if let Err(disposal) = unsafe { Pin::new_unchecked(&mut payload.future) }.destroy_pinned() {
            discard_task_panic_payload(disposal);
        }
        Self::retire_payload(maybe_payload);
        let shutdown = shared.shutdown();
        let (action, panic) = shared.panic_policy().resolve(panic);
        if action == crate::PanicAction::Shutdown {
            _ = shutdown.publish(ShutdownCause::panic(panic.as_ref()));
        }
        let cause = shutdown.is_triggered().then(|| shutdown.cause());
        result_target.panicked(panic, cause);
        self.metadata.report_panicked(&self.sink, self.poll_count.get());
        task::Poll::Ready(())
    }

    fn finish_terminal(self: Pin<&Self>, maybe_payload: &mut Option<Payload<F, R, T, I>>, outcome: TerminalOutcome<R>) {
        let Some(payload) = maybe_payload.as_mut() else {
            debug_assert!(matches!(outcome, TerminalOutcome::Cancelled(_)));
            return;
        };
        let result_target = payload
            .result_target
            .take()
            .expect("the result target is present until the task enters a terminal state");
        // #[gamma::skip(cond.always_false, reason = "On success FutureStorage already retired its future; destroying that empty storage returns Ok without running a destructor.")]
        let future_panic = if matches!(&outcome, TerminalOutcome::Succeeded(_)) {
            // FutureStorage retires the future before returning Ready.
            None
        } else {
            // SAFETY: The future is pinned with Task and destroyed in place.
            unsafe { Pin::new_unchecked(&mut payload.future) }.destroy_pinned().err()
        };
        Self::retire_payload(maybe_payload);

        match outcome {
            TerminalOutcome::Succeeded(result) => {
                result_target.completed(result);
                self.metadata.report_succeeded(&self.sink, self.poll_count.get());
            }
            TerminalOutcome::Cancelled(cause) => {
                if future_panic.is_some() {
                    self.metadata.report_panicked(&self.sink, self.poll_count.get());
                }
                result_target.cancelled(cause);
                if let Some(panic) = future_panic {
                    discard_task_panic_payload(panic);
                }
            }
        }
    }

    fn retire_payload(maybe_payload: &mut Option<Payload<F, R, T, I>>) {
        // The future is retired and the completion target has been transferred.
        // Diagnostic captures must not prevent delivery of that terminal outcome.
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
            // #[gamma::skip(assign_value.default, reason = "Option::default is None, exactly the assigned value.")]
            *maybe_payload = None;
        })) {
            discard_task_panic_payload(panic);
        }
    }
}

#[cfg_attr(
    not(test),
    expect(clippy::needless_pass_by_value, reason = "semantically correct to consume the panic")
)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn on_unhandled_task_panic(panic: TaskPanicPayload) -> ! {
    #[cfg(test)]
    {
        UNHANDLED_PANICS.set(UNHANDLED_PANICS.get() + 1);
        std::panic::resume_unwind(panic);
    }

    #[cfg(not(test))]
    if let Some(s) = panic.downcast_ref::<&str>() {
        eprintln!("unhandled panic in async task - terminating process: {s}");
    } else {
        eprintln!("unhandled panic in async task - terminating process");
    }

    #[cfg(not(test))]
    std::process::abort();
}

#[cfg(test)]
thread_local! {
    static UNHANDLED_PANICS: Cell<usize> = const { Cell::new(0) };
}

/// Stores the future and completion target for a task.
struct Payload<F, R, T, I>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
    I: Inspector<F>,
{
    /// The future that will be polled by the executor to progress the task.
    future: FutureStorage<F>,

    /// The task owns its completion target until it publishes a terminal outcome.
    result_target: Option<T>,
    #[cfg(feature = "inspection")]
    inspector: I,
    #[cfg(not(feature = "inspection"))]
    _inspector: I,
}

enum TerminalOutcome<R> {
    Succeeded(R),
    Cancelled(ShutdownCause),
}

/// Delivers a terminal task result. Each method consumes the target.
pub trait TaskCompletion<R>: Debug + 'static {
    /// Publishes a successful task result.
    ///
    /// Implementations must not panic.
    fn completed(self, result: R);

    /// Publishes a captured task panic. Implementations must not panic.
    ///
    /// `cause` contains the executor's first shutdown cause, or `None` if the
    /// executor continues running. The executor records the cause before this
    /// notification.
    fn panicked(self, panic: TaskPanicPayload, cause: Option<ShutdownCause>);

    /// Publishes task cancellation caused by executor shutdown.
    ///
    /// Implementations must not panic.
    fn cancelled(self, cause: ShutdownCause);
}

#[pin_project(PinnedDrop)]
struct FutureStorage<F> {
    #[pin]
    inner: Option<F>,
}

impl<F> FutureStorage<F> {
    fn new(inner: F) -> Self {
        Self { inner: Some(inner) }
    }

    fn destroy_pinned(self: Pin<&mut Self>) -> Result<(), TaskPanicPayload> {
        let mut inner = self.project().inner;
        // #[gamma::skip(cond.always_false, reason = "Setting an already empty pinned Option to None is a no-op and returns Ok.")]
        if inner.is_none() {
            return Ok(());
        }
        catch_unwind(AssertUnwindSafe(|| inner.set(None)))
    }
}

impl<F: Future> Future for FutureStorage<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let outcome = self
            .as_mut()
            .project()
            .inner
            .as_pin_mut()
            .expect("task future must not be polled after completion")
            .poll(cx);
        if outcome.is_ready()
            && let Err(panic) = self.destroy_pinned()
        {
            std::panic::resume_unwind(panic);
        }
        outcome
    }
}

#[pinned_drop]
impl<F> PinnedDrop for FutureStorage<F> {
    fn drop(self: Pin<&mut Self>) {
        if let Err(panic) = self.destroy_pinned() {
            on_unhandled_task_panic(panic);
        }
    }
}

pub(crate) trait TypeErasedTask: Send + Sync {
    /// Polls the task future.
    ///
    /// The task delivers its result separately. Returns `Poll::Ready(())` when
    /// the task completes or panics, and `Poll::Pending` otherwise.
    ///
    /// Do not poll a completed task. Completion drops the state that polling needs.
    ///
    /// # Panics
    ///
    /// Panics if the task has not been initialized.
    ///
    /// Panics if the task has already been completed.
    fn poll(self: Pin<&Self>, shared: &ExecutorShared) -> task::Poll<()>;

    /// Captures independently owned data for task-created telemetry.
    fn created_reporter(&self) -> Option<TaskCreatedReporter>;

    /// Returns owner-thread intrusive-list links for this task.
    fn links(&self) -> &TaskLinks;

    /// Returns storage for the immutable handle template embedded in this task.
    fn handle_slot(&self) -> &TaskHandleSlot;

    /// Records that the executor drained this task's queued wake notification.
    fn clear_queued_notification(&self);

    /// Promotes an active remote notification to local scheduling.
    fn promote_remote_notification(&self) -> bool;

    /// Drops the task payload and prevents more polling.
    ///
    /// Retained wakers keep the empty task allocation alive but become inert.
    fn abort(self: Pin<&Self>, cause: ShutdownCause);

    /// Initializes the task with its wake signal.
    ///
    /// # Panics
    ///
    /// Panics if called more than once.
    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal);

    // Keep optional diagnostics last so they do not shift ordinary hot-path vtable entries.
    #[cfg(feature = "inspection")]
    fn inspect(&self, state: TaskState, visitor: &mut dyn FnMut(&dyn Any));

    #[cfg(feature = "inspection")]
    fn is_notified(&self) -> bool;
}

#[cfg(test)]
// The executor prevents repolling retired tasks and repeated pinned-storage destruction.
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Waker};

    use super::*;

    #[test]
    fn nov4_pinned_storage_drop_dispatches_a_retirement_panic_to_the_unhandled_adapter() {
        struct Panics;
        impl Drop for Panics {
            fn drop(&mut self) {
                panic!("retiring pinned storage");
            }
        }
        UNHANDLED_PANICS.set(0);
        let storage = Box::pin(FutureStorage::new(Panics));
        let panic = catch_unwind(AssertUnwindSafe(|| drop(storage))).unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"retiring pinned storage"));
        assert_eq!(UNHANDLED_PANICS.get(), 1);
    }

    #[test]
    fn gamma_survivor_task_lifecycle_counts_real_polls_and_retires_saved_waker_before_delivery() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        use observed_testing::{TEST_ID, test_emitter};

        for panics in [false, true] {
            let pool = MultiPool::new();
            let polls = Rc::new(Cell::new(0));
            let saved = Rc::new(RefCell::new(None));
            let (sink, processor) = test_emitter(TEST_ID);
            let future = std::future::poll_fn({
                let polls = Rc::clone(&polls);
                let saved = Rc::clone(&saved);
                move |cx| {
                    *saved.borrow_mut() = Some(cx.waker().clone());
                    polls.set(polls.get() + 1);
                    if polls.get() < 3 {
                        return task::Poll::Pending;
                    }
                    assert!(!panics, "poll panic");
                    task::Poll::Ready(())
                }
            });
            let handle = TaskHandle::new(allocate_task(&pool, future, TaskMetadata::default(), sink, TestCompletion, ()));
            let owner = std::thread::current().id();
            let (wake, receiver) = crate::WakeShared::new(Waker::noop().clone(), owner);
            let shared = ExecutorShared::new(Waker::noop().clone(), Sink::noop(), owner, crate::PanicPolicy::IGNORE);
            // SAFETY: handle owns its initialized pinned task on this thread.
            let task = unsafe { handle.as_task() };
            task.initialize(WakeSignal::new(wake.clone(), handle.as_ptr()));
            assert!(task.poll(&shared).is_pending());
            assert!(task.poll(&shared).is_pending());
            assert!(task.poll(&shared).is_ready());
            let events = processor.events();
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].name(),
                if panics {
                    "arty.rt.task.panicked"
                } else {
                    "arty.rt.task.succeeded"
                }
            );
            assert_eq!(events[0].field_metrics()[0].value().to_bits(), 3.0_f64.to_bits());
            saved.borrow().as_ref().unwrap().wake_by_ref();
            assert!(!wake.has_remote_wakes());
            receiver.try_recv().unwrap_err();
            assert!(!task.promote_remote_notification());
            task.abort(ShutdownCause::default());
            drop(handle);
            assert_eq!(pool.len(), 1);
            saved.borrow_mut().take();
            assert!(pool.is_empty());
        }
    }

    #[test]
    fn gamma_survivor_cancellation_clears_payload_once_without_false_panic_telemetry() {
        use observed_testing::{TEST_ID, test_emitter};

        let pool = MultiPool::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let (sink, processor) = test_emitter(TEST_ID);
        let handle = TaskHandle::new(allocate_task(
            &pool,
            DropCountingFuture(Arc::clone(&drops)),
            TaskMetadata::default(),
            sink,
            TestCompletion,
            (),
        ));
        // SAFETY: handle owns its pinned task on this thread.
        let task = unsafe { handle.as_task() };
        task.abort(ShutdownCause::default());
        task.abort(ShutdownCause::default());
        drop(handle);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(processor.events().len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn gamma_survivor_cancellation_disposes_panicking_future_and_payload_once_and_reports_zero_polls() {
        use observed_testing::{TEST_ID, test_emitter};

        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                panic!("panic payload destructor");
            }
        }
        struct Pending(Arc<AtomicUsize>, Arc<AtomicUsize>);
        impl Future for Pending {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> task::Poll<()> {
                task::Poll::Pending
            }
        }
        impl Drop for Pending {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                std::panic::panic_any(Payload(Arc::clone(&self.1)));
            }
        }
        let pool = MultiPool::new();
        let future_drops = Arc::new(AtomicUsize::new(0));
        let payload_drops = Arc::new(AtomicUsize::new(0));
        let (sink, processor) = test_emitter(TEST_ID);
        let handle = TaskHandle::new(allocate_task(
            &pool,
            Pending(Arc::clone(&future_drops), Arc::clone(&payload_drops)),
            TaskMetadata::default(),
            sink,
            TestCompletion,
            (),
        ));
        // SAFETY: the handle owns its initialized pinned task on this thread.
        let task = unsafe { handle.as_task() };
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| task.abort(ShutdownCause::default())));
        result.unwrap();
        task.abort(ShutdownCause::default());
        drop(handle);
        assert_eq!(future_drops.load(Ordering::Relaxed), 1);
        assert_eq!(payload_drops.load(Ordering::Relaxed), 1);
        let event = processor.single_event();
        assert_eq!(event.name(), "arty.rt.task.panicked");
        assert_eq!(event.field_metrics()[0].value().to_bits(), 0.0_f64.to_bits());
        assert!(pool.is_empty());
    }

    #[test]
    fn gamma_survivor_poll_panic_disposes_secondary_future_drop_payload_without_unwinding() {
        use observed_testing::{TEST_ID, test_emitter};

        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                panic!("secondary payload destructor");
            }
        }
        struct Panicking(Arc<AtomicUsize>);
        impl Future for Panicking {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> task::Poll<()> {
                panic!("poll panic");
            }
        }
        impl Drop for Panicking {
            fn drop(&mut self) {
                std::panic::panic_any(Payload(Arc::clone(&self.0)));
            }
        }
        let pool = MultiPool::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let (sink, processor) = test_emitter(TEST_ID);
        let handle = TaskHandle::new(allocate_task(
            &pool,
            Panicking(Arc::clone(&drops)),
            TaskMetadata::default(),
            sink,
            TestCompletion,
            (),
        ));
        let owner = std::thread::current().id();
        let (wake, _receiver) = crate::WakeShared::new(Waker::noop().clone(), owner);
        let shared = ExecutorShared::new(Waker::noop().clone(), Sink::noop(), owner, crate::PanicPolicy::IGNORE);
        // SAFETY: handle owns its initialized pinned task on this thread.
        let task = unsafe { handle.as_task() };
        task.initialize(WakeSignal::new(wake, handle.as_ptr()));
        assert!(
            std::panic::catch_unwind(AssertUnwindSafe(|| task.poll(&shared)))
                .unwrap()
                .is_ready()
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        let event = processor.single_event();
        assert_eq!(event.name(), "arty.rt.task.panicked");
        assert_eq!(event.field_metrics()[0].value().to_bits(), 1.0_f64.to_bits());
        drop(handle);
        assert!(pool.is_empty());
    }

    fn initialized_task<R: 'static>(pool: &MultiPool, future: impl Future<Output = R> + 'static) -> TaskHandle {
        let handle = TaskHandle::new(allocate_task(
            pool,
            future,
            TaskMetadata::default(),
            Sink::noop(),
            TestCompletion,
            (),
        ));
        let (shared, _remote_wakes) = crate::WakeShared::new(Waker::noop().clone(), std::thread::current().id());
        let signal = WakeSignal::new(shared, handle.as_ptr());
        // SAFETY: The handle owns the task on its creating thread.
        unsafe { handle.as_task() }.initialize(signal);
        handle
    }

    #[test]
    fn abort_is_idempotent_after_the_payload_is_cleared() {
        let pool = MultiPool::new();
        let handle = initialized_task(&pool, std::future::pending::<()>());
        // SAFETY: The handle owns the task on its creating thread.
        let task = unsafe { handle.as_task() };

        task.abort(ShutdownCause::default());
        task.abort(ShutdownCause::default());

        drop(handle);
        assert!(pool.is_empty());
    }

    #[test]
    fn gamma_round2_aborting_an_unqueued_pending_task_makes_its_retained_waker_inert() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let owner = std::thread::current().id();
        crate::executor_context::register(owner);
        let captured = Rc::new(RefCell::new(None));
        let observed = Rc::clone(&captured);
        let pool = MultiPool::new();
        let handle = initialized_task(
            &pool,
            std::future::poll_fn(move |cx| {
                *observed.borrow_mut() = Some(cx.waker().clone());
                task::Poll::<()>::Pending
            }),
        );
        let shared = ExecutorShared::new(Waker::noop().clone(), Sink::noop(), owner, crate::PanicPolicy::IGNORE);
        // SAFETY: The handle owns its initialized pinned task on this thread.
        let task = unsafe { handle.as_task() };
        assert!(task.poll(&shared).is_pending());
        task.abort(ShutdownCause::default());
        captured.borrow().as_ref().unwrap().wake_by_ref();
        let was_requeued = crate::executor_context::has_wakes(owner);
        crate::executor_context::clear_wakes(owner);
        crate::executor_context::unregister(owner);
        drop(handle);
        drop(captured);
        assert!(!was_requeued);
        assert!(pool.is_empty());
    }

    #[test]
    fn polling_after_completion_panics() {
        let pool = MultiPool::new();
        let handle = initialized_task(&pool, std::future::ready(()));
        let shared = ExecutorShared::new(
            Waker::noop().clone(),
            Sink::noop(),
            std::thread::current().id(),
            crate::PanicPolicy::IGNORE,
        );
        // SAFETY: The handle owns the task on its creating thread.
        let task = unsafe { handle.as_task() };

        assert!(task.poll(&shared).is_ready());
        catch_unwind(AssertUnwindSafe(|| task.poll(&shared))).unwrap_err();
    }

    struct DropCountingFuture(Arc<AtomicUsize>);

    impl Future for DropCountingFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> task::Poll<Self::Output> {
            task::Poll::Pending
        }
    }

    impl Drop for DropCountingFuture {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "inspection")]
    #[test]
    fn uninitialized_task_can_be_aborted_and_has_no_description() {
        let pool = MultiPool::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let handle = allocate_task(
            &pool,
            DropCountingFuture(Arc::clone(&drops)),
            TaskMetadata::default(),
            Sink::noop(),
            TestCompletion,
            (),
        );
        let task = handle.as_ref();
        let mut descriptions = 0;

        task.abort(ShutdownCause::default());
        task.inspect(TaskState::Ready, &mut |_| descriptions += 1);
        drop(handle);

        assert_eq!((drops.load(Ordering::Relaxed), descriptions), (1, 0));
        assert!(pool.is_empty());
    }

    #[test]
    fn future_storage_destroys_the_future_exactly_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let mut storage = pin!(FutureStorage::new(DropCountingFuture(Arc::clone(&drops))));

            storage.as_mut().destroy_pinned().unwrap();
            storage.as_mut().destroy_pinned().unwrap();
        }

        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn future_storage_retires_a_panicking_destructor_exactly_once() {
        struct PanickingDropFuture(Arc<AtomicUsize>);

        impl Future for PanickingDropFuture {
            type Output = ();

            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> task::Poll<Self::Output> {
                task::Poll::Ready(())
            }
        }

        impl Drop for PanickingDropFuture {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                panic!("future destructor panic");
            }
        }

        for completes in [false, true] {
            let drops = Arc::new(AtomicUsize::new(0));
            {
                let mut storage = pin!(FutureStorage::new(PanickingDropFuture(Arc::clone(&drops))));
                if completes {
                    catch_unwind(AssertUnwindSafe(|| storage.as_mut().poll(&mut Context::from_waker(Waker::noop())))).unwrap_err();
                } else {
                    storage.as_mut().destroy_pinned().unwrap_err();
                }
                storage.as_mut().destroy_pinned().unwrap();
            }

            assert_eq!(drops.load(Ordering::Relaxed), 1);
        }
    }
}
