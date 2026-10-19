// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(all(debug_assertions, test))]
use std::backtrace::Backtrace;
use std::cell::{Cell, OnceCell, UnsafeCell};
use std::fmt::Debug;
use std::marker::PhantomPinned;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
#[cfg(all(debug_assertions, test))]
use std::sync::Arc;
use std::task;

use events_once::PooledLocalSender;
use observed::Sink;
use pin_project::{pin_project, pinned_drop};

use crate::remote::Sender as RemoteSender;
#[cfg(all(debug_assertions, test))]
use crate::{DiagnosticWaker, DiagnosticWakerRegistry};
use crate::{TaskCreatedReporter, TaskHandleSlot, TaskOptions, TaskPanicPayload, TaskPtr, WakeSignal, discard_task_panic_payload};

#[derive(Debug, Default)]
pub(crate) struct TaskLinks {
    pub(crate) previous: Cell<Option<TaskPtr>>,
    pub(crate) next: Cell<Option<TaskPtr>>,
    pub(crate) linked: Cell<bool>,
}

/// A task registered with the async task executor, in its fully typed form.
///
/// This facilitates the executor's internal bookkeeping and state management around a task.
/// The executor itself typically handles the task in obscured form, as a [`TypeErasedTask`],
/// as it does not care about the specific type information - only the task itself does.
#[derive(Debug)]
pub(crate) struct Task<F, R, T = PooledLocalSender<R>>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
{
    /// `None` if the task has either completed or has been abandoned by the executor (e.g. because
    /// it is shutting down).
    ///
    /// We use `UnsafeCell` to allow the `Task` to be accessed from multiple viewpoints, with the
    /// executor using both the payload and the wake signal, and the wakers emitted by the
    /// wake signal using the wake signal. All access is via shared references to ensure no
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

    /// Unit tests store backtraces of waker clones here to exercise leak diagnostics.
    #[cfg(all(debug_assertions, test))]
    diagnostic_waker_registry: Arc<DiagnosticWakerRegistry>,

    _requires_pin: PhantomPinned,
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "remote owners retain only the allocation; the executor retires non-Send payloads on their owner thread"
)]
// SAFETY: Task cells may be retained and reference-counted from remote threads, but task access is
// private and owner-thread-only. Cross-thread wakers touch only separately synchronized wake
// metadata. Completion and cancellation empty the non-Send payload before the owner releases its
// final scheduling reference, so remote final reclamation cannot drop user state.
unsafe impl<F, R, T> Send for Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
{
}

// SAFETY: Same invariant as `Send`; shared remote access never reaches task payload fields.
unsafe impl<F, R, T> Sync for Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
{
}

impl<F, R> Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new(future: F, result_tx: PooledLocalSender<R>) -> Self {
        Self::new_with_options(future, result_tx, TaskOptions::default(), Sink::noop())
    }

    #[must_use]
    pub(crate) fn new_with_options(future: F, result_tx: PooledLocalSender<R>, options: TaskOptions, sink: Sink) -> Self {
        Self::new_with_target(future, result_tx, options, sink)
    }

    #[cfg(test)]
    fn has_payload(&self) -> bool {
        // SAFETY: Tests inspect this only on the task's owner thread.
        unsafe { self.payload.get().as_ref() }
            .expect("UnsafeCell pointer cannot be null")
            .is_some()
    }
}

impl<F, R> Task<F, R, RemoteSender<R>>
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
    pub(crate) fn new_remote(future: F, options: TaskOptions, sink: Sink, remote_result_tx: RemoteSender<R>) -> Self {
        Self::new_with_target(future, remote_result_tx, options, sink)
    }
}

impl<F, R, T> Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
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
            #[cfg(all(debug_assertions, test))]
            diagnostic_waker_registry: Arc::new(DiagnosticWakerRegistry::new()),
            _requires_pin: PhantomPinned,
        }
    }
}

impl<F, R, T> TypeErasedTask for Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
{
    fn poll(self: Pin<&Self>) -> task::Poll<()> {
        self.poll_count.set(self.poll_count.get().saturating_add(1));
        self.options.with_context(|| self.poll_in_context())
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
    fn abort(self: Pin<&Self>) {
        if let Some(wake_signal) = self.wake_signal.get() {
            wake_signal.retire();
        }

        // SAFETY: The `Task` is single-threaded and we only ever create temporary references
        // to `payload` that do not escape `Task` methods, so we know there cannot be a conflicting
        // reference to this field.
        let maybe_payload = unsafe { self.payload.get().as_mut() }.expect("UnsafeCell pointer cannot be null");

        self.options
            .with_context(|| self.finish_terminal(maybe_payload, TerminalOutcome::Cancelled));
    }

    fn initialize(self: Pin<&Self>, wake_signal: WakeSignal) {
        self.wake_signal.set(wake_signal).expect("task must be initialized exactly once");
    }

    #[cfg(all(debug_assertions, test))]
    fn inspect_waker_backtraces(&self, f: &mut dyn FnMut(&Backtrace)) {
        self.diagnostic_waker_registry.inspect_backtraces(f);
    }
}

impl<F, R, T> Task<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
{
    fn pinned_wake_signal(self: Pin<&Self>) -> Pin<&WakeSignal> {
        let signal = self.get_ref().wake_signal.get().expect("task must be initialized before poll()");
        // SAFETY: OnceCell never removes or replaces the signal, which stays pinned with Task.
        unsafe { Pin::new_unchecked(signal) }
    }

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

        let wake_signal = self.pinned_wake_signal();

        let poll_result = {
            // SAFETY: The borrowed polling waker cannot escape this block. Clones retain the
            // pinned pooled task allocation; owner retirement empties its payload before a remote
            // clone can release the final reference.
            let waker = unsafe { wake_signal.waker_ref() };

            // Unit tests wrap the waker to exercise clone backtrace diagnostics.
            #[cfg(all(debug_assertions, test))]
            let waker = DiagnosticWaker::with_inner_and_registry(waker.clone(), Arc::clone(&self.diagnostic_waker_registry));

            let mut cx = task::Context::from_waker(&waker);

            // Runtime integrations can provide a remote result target that receives captured
            // panics. Plain executor tasks retain the fail-fast behavior.
            match catch_unwind(AssertUnwindSafe(|| future_as_mut_pinned.poll(&mut cx))) {
                Ok(x) => x,
                Err(panic) => return self.finish_panicked(maybe_payload, wake_signal, panic),
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
    ) -> task::Poll<()> {
        let payload = maybe_payload.as_mut().expect("a polled task retains its payload");
        let result_is_remote = payload
            .result_target
            .as_ref()
            .expect("a polled task retains its result target")
            .is_remote();
        if !result_is_remote {
            // Keep the local sender on the fail-fast unwind path so its receiver disconnects.
            let _result_target = payload.result_target.take().expect("a polled task retains its result target");
            on_unhandled_task_panic(panic);
        }

        wake_signal.retire();
        self.finish_terminal(maybe_payload, TerminalOutcome::Panicked(panic));
        task::Poll::Ready(())
    }

    fn finish_terminal(self: Pin<&Self>, maybe_payload: &mut Option<Payload<F, R, T>>, outcome: TerminalOutcome<R>) {
        let Some(payload) = maybe_payload.as_mut() else {
            debug_assert!(matches!(outcome, TerminalOutcome::Cancelled));
            return;
        };
        let result_target = payload
            .result_target
            .take()
            .expect("the result target is present until the task enters a terminal state");
        let remote = result_target.is_remote();

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
                if let Err(panic) = result_target.complete(result) {
                    if remote {
                        discard_task_panic_payload(panic);
                    } else {
                        self.options.report_panicked(&self.sink, self.poll_count.get());
                        on_unhandled_task_panic(panic);
                    }
                }
                self.options.report_succeeded(&self.sink, self.poll_count.get());
            }
            TerminalOutcome::Panicked(panic) => {
                debug_assert!(remote, "plain task panics must follow the fail-fast path");
                if let Some(disposal) = future_panic {
                    discard_task_panic_payload(disposal);
                }
                if let Err(disposal) = result_target.panicked(panic) {
                    discard_task_panic_payload(disposal);
                }
                self.options.report_panicked(&self.sink, self.poll_count.get());
            }
            TerminalOutcome::Cancelled => {
                if future_panic.is_some() {
                    self.options.report_panicked(&self.sink, self.poll_count.get());
                }
                let result_panic = catch_unwind(AssertUnwindSafe(|| drop(result_target))).err();
                for panic in [future_panic, result_panic].into_iter().flatten() {
                    if remote {
                        discard_task_panic_payload(panic);
                    } else {
                        on_unhandled_task_panic(panic);
                    }
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
fn on_unhandled_task_panic(panic: TaskPanicPayload) -> ! {
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

/// Deals with the payload elements of a task - the future and the result.
struct Payload<F, R, T>
where
    F: Future<Output = R> + 'static,
    R: 'static,
    T: TaskResultTarget<R>,
{
    /// The future that will be polled by the executor to progress the task.
    future: FutureStorage<F>,

    /// The result sender/receiver use pooled event storage whose detached slots are retained by
    /// their endpoints. Join handles may therefore outlive both the task and the executor.
    result_target: Option<T>,
}

enum TerminalOutcome<R> {
    Succeeded(R),
    Panicked(TaskPanicPayload),
    Cancelled,
}

/// Inline result-delivery policy owned by a task.
///
/// Local targets keep raw executor task panics fail-fast. Remote targets transport captured task
/// panics to another thread and contain panics raised while notifying or disposing that endpoint.
/// Publishing consumes the target. A returned panic payload comes from delivery or disposal and
/// does not imply that the task outcome was not already published.
pub(crate) trait TaskResultTarget<R>: Debug + 'static {
    /// Returns whether this target transports captured task panics instead of terminating.
    fn is_remote(&self) -> bool;

    /// Publishes a successful task result, returning any panic raised by delivery.
    fn complete(self, result: R) -> Result<(), TaskPanicPayload>;

    /// Publishes a captured task panic, returning a distinct panic raised by delivery.
    fn panicked(self, panic: TaskPanicPayload) -> Result<(), TaskPanicPayload>;
}

impl<R: 'static> TaskResultTarget<R> for PooledLocalSender<R> {
    fn is_remote(&self) -> bool {
        false
    }

    fn complete(self, result: R) -> Result<(), TaskPanicPayload> {
        catch_unwind(AssertUnwindSafe(|| self.send(result)))
    }

    fn panicked(self, _panic: TaskPanicPayload) -> Result<(), TaskPanicPayload> {
        unreachable!("local task panics terminate the process")
    }
}

impl<R: Send + 'static> TaskResultTarget<R> for RemoteSender<R> {
    fn is_remote(&self) -> bool {
        true
    }

    fn complete(self, result: R) -> Result<(), TaskPanicPayload> {
        self.completed(result)
    }

    fn panicked(self, panic: TaskPanicPayload) -> Result<(), TaskPanicPayload> {
        self.panicked(panic)
    }
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
    fn drop(self: Pin<&mut Self>) {
        if let Err(panic) = self.destroy_pinned() {
            on_unhandled_task_panic(panic);
        }
    }
}

pub(crate) trait TypeErasedTask: Send + Sync {
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

    /// Clears the inner state of the task, entering a form where no further polling is possible
    /// and any future-specific state is dropped.
    ///
    /// Retained wakers keep the empty task allocation alive but become inert.
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::{Ready, pending};
    use std::pin::pin;
    use std::rc::Rc;
    #[cfg(not(debug_assertions))]
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Wake, Waker};

    use events_once::{Disconnected, LocalEventPool};
    use observed_testing::{TEST_ID, test_emitter};
    use static_assertions::assert_impl_all;
    use testing_aids::assert_panic;

    use super::*;
    use crate::TaskHandle;
    use crate::remote::Pool as RemotePool;
    use crate::testing::TestSubjectFuture;

    fn fake_wake_owner() -> TaskHandle {
        crate::testing::new_test_task_handle()
    }

    fn fake_wake_signal(owner: &TaskHandle) -> WakeSignal {
        WakeSignal::new_for_test(Waker::noop().clone(), owner)
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
        let wake_owner = fake_wake_owner();
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
        task.initialize(fake_wake_signal(&wake_owner));

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
    fn wake_signal_is_initialized_exactly_once() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();
        let task = pin!(Task::new(pending::<u64>(), tx));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));

        assert_panic!(task.initialize(fake_wake_signal(&wake_owner)));

        assert!(task.wake_signal.get().is_some());
        task.abort();
    }

    #[test]
    fn clear_queued_notification_forwards_to_the_wake_signal() {
        let _executor = crate::Executor::builder().build();
        let fake_task_handle = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();
        let task = pin!(Task::new(pending::<u64>(), tx));
        let task = task.as_ref();
        let signal = WakeSignal::new_for_test(Waker::noop().clone(), &fake_task_handle);
        task.initialize(signal);
        let signal = task.pinned_wake_signal();
        // SAFETY: The pinned signal remains alive for this borrow.
        let waker = unsafe { signal.waker_ref() };
        waker.wake_by_ref();
        assert!(signal.has_queued_notification());
        let mut queued = std::collections::VecDeque::new();
        signal.append_test_wakes(&mut queued);

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

    struct DropCountingFuture(std::sync::Arc<AtomicUsize>);

    impl Future for DropCountingFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> task::Poll<Self::Output> {
            task::Poll::Pending
        }
    }

    impl Drop for DropCountingFuture {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn future_storage_destroy_pinned_destroys_exactly_once() {
        let drops = std::sync::Arc::new(AtomicUsize::new(0));
        {
            let mut storage = pin!(FutureStorage::new(DropCountingFuture(std::sync::Arc::clone(&drops))));

            storage.as_mut().destroy_pinned().unwrap();
            assert_eq!(drops.load(Ordering::Relaxed), 1);
        }
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn future_storage_drop_destroys_live_future() {
        let drops = std::sync::Arc::new(AtomicUsize::new(0));

        drop(FutureStorage::new(DropCountingFuture(std::sync::Arc::clone(&drops))));

        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn future_storage_retires_a_panicking_destructor_exactly_once() {
        struct PanickingDropFuture(std::sync::Arc<AtomicUsize>);

        impl Future for PanickingDropFuture {
            type Output = ();

            fn poll(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> task::Poll<Self::Output> {
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
            let drops = std::sync::Arc::new(AtomicUsize::new(0));
            {
                let mut storage = pin!(FutureStorage::new(PanickingDropFuture(std::sync::Arc::clone(&drops))));
                if completes {
                    assert_panic!(storage.as_mut().poll(&mut task::Context::from_waker(Waker::noop())));
                } else {
                    storage.as_mut().destroy_pinned().unwrap_err();
                }
                assert_eq!(drops.load(Ordering::Relaxed), 1);
                storage.as_mut().destroy_pinned().unwrap();
            }
            assert_eq!(drops.load(Ordering::Relaxed), 1);
        }
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

    struct PanicOnPollAndDropFuture;

    impl Future for PanicOnPollAndDropFuture {
        type Output = u64;

        fn poll(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
            panic!("panic from future poll");
        }
    }

    impl Drop for PanicOnPollAndDropFuture {
        fn drop(&mut self) {
            panic!("panic from future destructor");
        }
    }

    #[test]
    fn smoke_test() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(fake_wake_signal(&wake_owner));

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
        let wake_owner = fake_wake_owner();
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
            TaskOptions::observed(sink.transfer_context(), crate::TaskPlacement::Any),
            sink,
        ));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));
        task.created_reporter().unwrap().report();

        assert!(task.poll().is_pending());
        assert!(task.poll().is_ready());

        let events = processor.events();
        let completed = events.iter().find(|event| event.name() == "arty.rt.task.succeeded").unwrap();
        assert_eq!(completed.field_metrics()[0].field_key(), "polls");
        assert!((completed.field_metrics()[0].value() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn observed_remote_panic_reports_aggregate_poll_count() {
        let wake_owner = fake_wake_owner();
        let (result_tx, mut result_rx) = RemotePool::new().channel::<u64>();
        let (sink, processor) = test_emitter(TEST_ID);
        let polls = Rc::new(Cell::new(0));
        let task = pin!(Task::new_remote(
            std::future::poll_fn({
                let polls = Rc::clone(&polls);
                move |_| {
                    polls.set(polls.get() + 1);
                    assert_ne!(polls.get(), 2, "captured task panic");
                    task::Poll::Pending
                }
            }),
            TaskOptions::observed(sink.transfer_context(), crate::TaskPlacement::Any),
            sink,
            result_tx,
        ));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));
        task.created_reporter().unwrap().report();

        assert!(task.poll().is_pending());
        assert!(task.poll().is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());
        let task::Poll::Ready(Err(error)) = Pin::new(&mut result_rx).poll(&mut cx) else {
            panic!("captured panic must be published to the remote join handle");
        };
        assert!(error.is_panic());
        discard_task_panic_payload(error.into_panic().expect("the join error was checked as a panic"));

        let events = processor.events();
        let panicked = events.iter().find(|event| event.name() == "arty.rt.task.panicked").unwrap();
        assert_eq!(panicked.field_metrics()[0].field_key(), "polls");
        assert!((panicked.field_metrics()[0].value() - 2.0).abs() < f64::EPSILON);
        assert!(!events.iter().any(|event| event.name() == "arty.rt.task.succeeded"));
    }

    #[test]
    fn cancellation_emits_no_terminal_task_event() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();
        let (sink, processor) = test_emitter(TEST_ID);
        let task = pin!(Task::new_with_options(
            pending::<u64>(),
            tx,
            TaskOptions::observed(sink.transfer_context(), crate::TaskPlacement::Any),
            sink,
        ));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));
        task.created_reporter().unwrap().report();

        task.abort();
        task.abort();

        let events = processor.events();
        assert!(events.iter().any(|event| event.name() == "arty.rt.task.spawned"));
        assert!(
            !events
                .iter()
                .any(|event| { matches!(event.name(), "arty.rt.task.succeeded" | "arty.rt.task.panicked") })
        );
    }

    struct PanickingJoinWake;

    impl Wake for PanickingJoinWake {
        fn wake(self: Arc<Self>) {
            panic!("join notification panic");
        }
    }

    #[test]
    fn cancellation_join_notification_panic_is_not_reported_as_a_task_panic() {
        let wake_owner = fake_wake_owner();
        let (result_tx, mut result_rx) = RemotePool::new().channel::<u64>();
        let (sink, processor) = test_emitter(TEST_ID);
        let waker = Waker::from(Arc::new(PanickingJoinWake));
        assert!(Pin::new(&mut result_rx).poll(&mut task::Context::from_waker(&waker)).is_pending());
        let task = pin!(Task::new_remote(pending::<u64>(), TaskOptions::default(), sink, result_tx));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));
        task.created_reporter().unwrap().report();

        task.abort();

        let events = processor.events();
        assert!(events.iter().any(|event| event.name() == "arty.rt.task.spawned"));
        assert!(!events.iter().any(|event| event.name() == "arty.rt.task.panicked"));
    }

    #[test]
    fn local_completion_notification_panic_follows_fail_fast_path() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (result_tx, mut result_rx) = event_pool.rent();
        let waker = Waker::from(Arc::new(PanickingJoinWake));
        assert!(Pin::new(&mut result_rx).poll(&mut task::Context::from_waker(&waker)).is_pending());
        let task = pin!(Task::new(std::future::ready(42), result_tx));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));

        assert_panic!(task.poll());
    }

    #[test]
    fn remote_poll_and_destructor_panics_are_both_contained() {
        let wake_owner = fake_wake_owner();
        let (result_tx, mut result_rx) = RemotePool::new().channel::<u64>();
        let task = pin!(Task::new_remote(
            PanicOnPollAndDropFuture,
            TaskOptions::default(),
            Sink::noop(),
            result_tx,
        ));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));

        assert!(task.poll().is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());
        let task::Poll::Ready(Err(error)) = Pin::new(&mut result_rx).poll(&mut cx) else {
            panic!("captured poll panic must reach the remote join");
        };
        discard_task_panic_payload(error.into_panic().unwrap());
    }

    #[test]
    fn plain_task_panic_remains_unhandled() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<()>::new();
        let (tx, mut rx) = event_pool.rent();
        let task = pin!(Task::new(
            std::future::poll_fn(|_| -> task::Poll<()> {
                panic!("plain task panic");
            }),
            tx
        ));
        let task = task.as_ref();
        task.initialize(fake_wake_signal(&wake_owner));

        assert_panic!(task.poll());
        assert!(task.has_payload());
        let mut cx = task::Context::from_waker(Waker::noop());
        assert_eq!(Pin::new(&mut rx).poll(&mut cx), task::Poll::Ready(Err(Disconnected)));
    }

    #[test]
    fn local_result_target_rejects_panic_reporting() {
        let event_pool = LocalEventPool::<u64>::new();
        let (sender, _receiver) = event_pool.rent();

        assert_panic!(<PooledLocalSender<u64> as TaskResultTarget<u64>>::panicked(
            sender,
            Box::new("local task panic"),
        ));
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

        let wake_owner = fake_wake_owner();
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

        task.initialize(fake_wake_signal(&wake_owner));

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
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let shared_state = Rc::new(123);
        let shared_state_weak = Rc::downgrade(&shared_state);

        let task = pin!(Task::new(async move { *shared_state }, tx,));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(fake_wake_signal(&wake_owner));

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
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();

        let shared_state = Rc::new(123);
        let shared_state_weak = Rc::downgrade(&shared_state);

        let task = pin!(Task::new(async move { *shared_state }, tx,));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(fake_wake_signal(&wake_owner));

        let result = task.poll();
        assert!(matches!(result, task::Poll::Ready(())));

        // The state held by the task must have been dropped.
        assert!(shared_state_weak.upgrade().is_none());
    }

    #[test]
    fn panic_while_dropping_completed_future_is_contained() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let task = pin!(Task::new(PanicOnDropFuture(std::future::ready(42)), tx));
        let task = task.as_ref();

        task.initialize(fake_wake_signal(&wake_owner));

        assert_panic!(task.poll());

        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn panic_while_dropping_aborted_future_is_contained() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, mut rx) = event_pool.rent();

        let task = pin!(Task::new(PanicOnDropFuture(std::future::ready(42)), tx));
        let task = task.as_ref();

        task.initialize(fake_wake_signal(&wake_owner));

        assert_panic!(task.abort());

        let mut cx = task::Context::from_waker(Waker::noop());
        let event_result = Pin::new(&mut rx).poll(&mut cx);
        assert!(matches!(event_result, task::Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn poll_after_complete_panics() {
        let wake_owner = fake_wake_owner();
        let event_pool = LocalEventPool::<u64>::new();
        let (tx, _rx) = event_pool.rent();

        let task = pin!(Task::new(async { 42 }, tx));

        // Task is designed to be used via shared references.
        let task = task.as_ref();

        task.initialize(fake_wake_signal(&wake_owner));

        // The future is a trivial one that completes on the first poll.
        let task_result = task.poll();

        assert!(matches!(task_result, task::Poll::Ready(())));

        // A poll after completion is not legal.
        assert_panic!(task.poll());
    }

    #[test]
    fn thread_safety() {
        assert_impl_all!(Task<TestSubjectFuture, ()>: Send, Sync);
    }
}
