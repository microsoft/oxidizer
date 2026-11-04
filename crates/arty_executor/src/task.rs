// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, OnceCell, UnsafeCell};
use std::fmt::Debug;
use std::marker::PhantomPinned;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task;

use observed::Sink;
use pin_project::{pin_project, pinned_drop};
use plurality::MultiPool;

use crate::{
    ExecutorShared, ShutdownCause, TaskCreatedReporter, TaskHandle, TaskHandleSlot, TaskOptions, TaskPanicPayload, TaskPtr, WakeSignal,
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
struct Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    /// `None` after the task completes or the executor abandons it during shutdown.
    ///
    /// We use `UnsafeCell` to allow the `Task` to be accessed from multiple viewpoints, with the
    /// executor using both the payload and the wake signal, and the wakers emitted by the
    /// wake signal using the wake signal. All access is through shared references, so there are no
    /// aliasing violations, with `UnsafeCell` used when mutation of the contents is needed.
    /// (Outer) mutation of the fields only occurs by the executor, ensuring thread-safety.
    payload: UnsafeCell<Option<Payload<F, R, T>>>,

    /// Initialized once on the owner, before any waker is exposed. The signal is never replaced.
    wake_signal: OnceCell<WakeSignal>,

    handle_slot: TaskHandleSlot,

    options: TaskOptions,

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
unsafe impl<F, R, T> Send for Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
}

// SAFETY: This uses the same invariant as `Send`. Shared remote access never reaches task payload fields.
unsafe impl<F, R, T> Sync for Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
}

impl<F, R, T> Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    fn new_with_target(future: F, result_target: T, options: TaskOptions, sink: Sink) -> Self {
        Self {
            payload: UnsafeCell::new(Some(Payload {
                future: FutureStorage::new(future),
                result_target: Some(result_target),
            })),
            wake_signal: OnceCell::new(),
            handle_slot: TaskHandleSlot::new(),
            options,
            sink,
            poll_count: Cell::new(0),
            links: TaskLinks::default(),
            _requires_pin: PhantomPinned,
        }
    }
}

pub(crate) fn allocate_task<F, R, T>(pool: &MultiPool, future: F, options: TaskOptions, sink: Sink, completion: T) -> TaskHandle
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    TaskHandle::new(pool.alloc_arc_pin(Task::new_with_target(future, completion, options, sink)))
}

#[cfg(test)]
#[derive(Debug)]
struct TestCompletion;

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
    allocate_task(
        pool,
        std::future::pending::<()>(),
        TaskOptions::default(),
        Sink::noop(),
        TestCompletion,
    )
}

impl<F, R, T> TypeErasedTask for Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    fn poll(self: Pin<&Self>, shared: &ExecutorShared) -> task::Poll<()> {
        self.poll_count.set(self.poll_count.get().saturating_add(1));
        self.options.with_context(|| self.poll_in_context(shared))
    }

    fn created_reporter(&self) -> Option<TaskCreatedReporter> {
        self.options.created_reporter(self.sink.clone())
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

    #[cfg_attr(test, mutants::skip)] // Mutation causes resources not to be released, leading to executor shutdown never happening.
    fn abort(self: Pin<&Self>, cause: ShutdownCause) {
        if let Some(wake_signal) = self.wake_signal.get() {
            wake_signal.retire();
        }

        // SAFETY: The `Task` is single-threaded and we only ever create temporary references
        // to `payload` that do not escape `Task` methods, so we know there cannot be a conflicting
        // reference to this field.
        let maybe_payload = unsafe { self.payload.get().as_mut() }.expect("UnsafeCell pointer cannot be null");

        self.options
            .with_context(|| self.finish_terminal(maybe_payload, TerminalOutcome::Cancelled(cause)));
    }

    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        self.wake_signal.set(wake_signal).expect("task must be initialized exactly once");
    }
}

impl<F, R, T> Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
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
        maybe_payload: &mut Option<Payload<F, R, T>>,
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
        *maybe_payload = None;
        let shutdown = shared.shutdown();
        if matches!(shared.panic_policy(), crate::PanicPolicy::Shutdown) {
            _ = shutdown.publish(ShutdownCause::panic());
        }
        let cause = shutdown.is_triggered().then(|| shutdown.cause());
        result_target.panicked(panic, cause);
        self.options.report_panicked(&self.sink, self.poll_count.get());
        task::Poll::Ready(())
    }

    fn finish_terminal(self: Pin<&Self>, maybe_payload: &mut Option<Payload<F, R, T>>, outcome: TerminalOutcome<R>) {
        let Some(payload) = maybe_payload.as_mut() else {
            debug_assert!(matches!(outcome, TerminalOutcome::Cancelled(_)));
            return;
        };
        let result_target = payload
            .result_target
            .take()
            .expect("the result target is present until the task enters a terminal state");
        let future_panic = if matches!(&outcome, TerminalOutcome::Succeeded(_)) {
            // FutureStorage retires the future before returning Ready.
            None
        } else {
            // SAFETY: The future is pinned with Task and destroyed in place.
            unsafe { Pin::new_unchecked(&mut payload.future) }.destroy_pinned().err()
        };
        // Assignment drops the retired storage in place without moving its pinned future.
        *maybe_payload = None;

        match outcome {
            TerminalOutcome::Succeeded(result) => {
                result_target.completed(result);
                self.options.report_succeeded(&self.sink, self.poll_count.get());
            }
            TerminalOutcome::Cancelled(cause) => {
                if future_panic.is_some() {
                    self.options.report_panicked(&self.sink, self.poll_count.get());
                }
                result_target.cancelled(cause);
                if let Some(panic) = future_panic {
                    discard_task_panic_payload(panic);
                }
            }
        }
    }
}

#[cfg_attr(
    not(test),
    expect(clippy::needless_pass_by_value, reason = "semantically correct to consume the panic")
)]
#[cfg_attr(test, mutants::skip)] // Impractical to unit test process termination.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn on_unhandled_task_panic(panic: TaskPanicPayload) -> ! {
    #[cfg(test)]
    std::panic::resume_unwind(panic);

    #[cfg(not(test))]
    if let Some(s) = panic.downcast_ref::<&str>() {
        eprintln!("unhandled panic in async task - terminating process: {s}");
    } else {
        eprintln!("unhandled panic in async task - terminating process");
    }

    #[cfg(not(test))]
    std::process::abort();
}

/// Stores the future and completion target for a task.
struct Payload<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    /// The future that will be polled by the executor to progress the task.
    future: FutureStorage<F>,

    /// The task owns its completion target until it publishes a terminal outcome.
    result_target: Option<T>,
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
    #[cfg_attr(coverage_nightly, coverage(off))] // Wrapper-invariant fallback terminates in a child process.
    #[cfg_attr(test, mutants::skip)] // A no-op differs only in the process-terminating panicking-destructor fallback.
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
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Waker};

    use super::*;

    fn initialized_task<R: 'static>(pool: &MultiPool, future: impl Future<Output = R> + 'static) -> TaskHandle {
        let handle = allocate_task(pool, future, TaskOptions::default(), Sink::noop(), TestCompletion);
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
    fn polling_after_completion_panics() {
        let pool = MultiPool::new();
        let handle = initialized_task(&pool, std::future::ready(()));
        let shared = ExecutorShared::new(
            Waker::noop().clone(),
            Sink::noop(),
            std::thread::current().id(),
            crate::PanicPolicy::Ignore,
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
