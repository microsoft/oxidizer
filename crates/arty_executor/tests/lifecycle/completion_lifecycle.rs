// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public task-completion and executor-lifecycle contracts.

use std::cell::{Cell, RefCell};
use std::error::Error as _;
use std::fmt;
use std::future::{Ready, pending, poll_fn, ready};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use arty_executor::{
    CycleOutcome, Executor, PanicPolicy, Shutdown, ShutdownCause, ShutdownError, TaskCompletion, TaskOptions, TaskPanicPayload,
    TaskPlacement, TaskSet,
};
use observed::Sink;
use observed::enrichment::{Enrichment, EnrichmentEntry};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed_testing::{TEST_ID, test_emitter};

use crate::support::{CompletionHandle, TaskOutcome, TaskSetExt as _};

thread_local! {
    static THREAD_LOCAL_EXECUTOR: RefCell<Option<Executor>> = const { RefCell::new(None) };
    static REENTRANT_TELEMETRY_SUBMISSION: RefCell<Option<(TaskSet, Rc<Cell<bool>>)>> = const { RefCell::new(None) };
    static OWNER_DROP_TELEMETRY: RefCell<Option<OwnerDropState>> = const { RefCell::new(None) };
}

struct ReentrantSubmissionProcessor;
struct OwnerDropProcessor;

struct ContextValue;

impl data_privacy::RedactedDisplay for ContextValue {
    fn fmt(&self, _: &dyn data_privacy::Redactor, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

struct TaskContext;

impl Enrichment for TaskContext {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        vec![EnrichmentEntry::new("task.context", ContextValue)]
    }
}

struct CallerContext;

impl Enrichment for CallerContext {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        vec![EnrichmentEntry::new("caller.context", ContextValue)]
    }
}

struct OwnerDropState {
    owner: Executor,
    tasks: TaskSet,
    nested: Rc<RefCell<Option<CompletionHandle<usize>>>>,
}

impl EventProcessor for ReentrantSubmissionProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == "arty.rt.task.spawned"
    }

    fn process(&self, _event: &EventView<'_>) {
        let Some((tasks, completed)) = REENTRANT_TELEMETRY_SUBMISSION.take() else {
            return;
        };
        drop(tasks.add_test(
            async move {
                completed.set(true);
            },
            TaskOptions::default(),
        ));
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

impl EventProcessor for OwnerDropProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == "arty.rt.task.spawned"
    }

    fn process(&self, _event: &EventView<'_>) {
        let Some(state) = OWNER_DROP_TELEMETRY.take() else {
            return;
        };
        drop(state.owner);
        let nested = state.tasks.add_test(async { 42 }, TaskOptions::default());
        assert!(state.nested.replace(Some(nested)).is_none());
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

fn poll_once<R: 'static>(handle: &mut CompletionHandle<R>) -> Poll<TaskOutcome<R>> {
    Pin::new(handle).poll(&mut Context::from_waker(Waker::noop()))
}

#[expect(clippy::panic, reason = "test helper reports an unexpected completion state")]
fn poll_completed<R: 'static>(handle: &mut CompletionHandle<R>) -> R {
    let Poll::Ready(TaskOutcome::Completed(result)) = poll_once(handle) else {
        panic!("completion target must receive a result");
    };
    result
}

#[expect(clippy::panic, reason = "test helper reports an unexpected completion state")]
fn poll_cancelled<R: 'static>(handle: &mut CompletionHandle<R>) -> ShutdownCause {
    let Poll::Ready(TaskOutcome::Cancelled(cause)) = poll_once(handle) else {
        panic!("completion target must receive cancellation");
    };
    cause
}

#[derive(Debug)]
struct TestShutdownError(&'static str);

impl fmt::Display for TestShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for TestShutdownError {}

fn shutdown_cause(message: &'static str) -> (ShutdownCause, ShutdownError) {
    let error = ShutdownError::new(TestShutdownError(message));
    (ShutdownCause::from(error.clone()), error)
}

fn assert_same_shutdown_source(actual: &ShutdownError, expected: &ShutdownError) {
    let actual = actual
        .inner()
        .downcast_ref::<TestShutdownError>()
        .expect("test shutdown cause uses TestShutdownError");
    let expected = expected
        .inner()
        .downcast_ref::<TestShutdownError>()
        .expect("test shutdown cause uses TestShutdownError");
    assert!(std::ptr::eq(actual, expected));
}

#[test]
fn shutdown_cause_exposes_the_original_error() {
    let (cause, expected) = shutdown_cause("executor failed");
    let actual = cause.as_error().unwrap();

    assert_same_shutdown_source(actual, &expected);
    let source = actual.source().unwrap().downcast_ref::<TestShutdownError>().unwrap();
    let expected = expected.inner().downcast_ref::<TestShutdownError>().unwrap();
    assert!(std::ptr::eq(source, expected));
    assert_eq!(format!("{cause:?}"), "ShutdownCause { has_error: true }");
}

fn enrichment_keys(sink: &Sink) -> Vec<String> {
    sink.current_enrichments()
        .into_iter()
        .map(|entry| entry.key().as_str().to_owned())
        .collect()
}

#[test]
fn failed_construction_preserves_existing_executor_notifications() {
    let mut executor = Executor::builder().build();
    let ready = Rc::new(Cell::new(false));
    let retained = Rc::new(RefCell::new(None));
    let mut handle = executor.tasks().add_test(
        poll_fn({
            let ready = Rc::clone(&ready);
            let retained = Rc::clone(&retained);
            move |cx| {
                if ready.get() {
                    Poll::Ready(42)
                } else {
                    *retained.borrow_mut() = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }),
        TaskOptions::default(),
    );
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    ready.set(true);
    retained.borrow().as_ref().unwrap().wake_by_ref();

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Executor::builder().build())).unwrap_err();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut handle), 42);
}

#[test]
fn noop_executor_applies_task_context_and_restores_caller_context() {
    let sink = Sink::builder("arty-executor-context-restoration").build();
    let task_context = sink.transfer_context().with_enrichment(TaskContext);
    let caller_context = sink.transfer_context().with_enrichment(CallerContext);
    let _caller_guard = caller_context.apply_current_thread();
    assert_eq!(enrichment_keys(&sink), ["caller.context"]);

    let mut executor = Executor::builder().build();
    let task_sink = sink.clone();
    let mut handle = executor.tasks().add_test(
        async move { enrichment_keys(&task_sink) },
        TaskOptions::observed(task_context, TaskPlacement::Any),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut handle), [String::from("task.context")]);
    assert_eq!(enrichment_keys(&sink), ["caller.context"]);
}

#[test]
fn completed_result_remains_available_after_executor_stops() {
    let mut executor = Executor::builder().build();
    let mut handle = executor.tasks().add_test(async { 42 }, TaskOptions::default());

    assert!(executor.has_tasks());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert!(!executor.has_tasks());
    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Completed(42))));
}

#[test]
fn task_panic_publishes_the_original_payload_and_keeps_the_executor_usable() {
    let (sink, processor) = test_emitter(TEST_ID);
    let mut executor = Executor::builder().sink(sink.clone()).panic_policy(PanicPolicy::Ignore).build();
    let tasks = executor.tasks();
    let mut panicked = tasks.add_test(
        poll_fn(|_| -> Poll<usize> { panic!("task panic") }),
        TaskOptions::observed(sink.transfer_context(), TaskPlacement::Any),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut panicked) else {
        panic!("panicking task must publish its panic payload");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"task panic"));
    assert!(processor.events().iter().any(|event| event.name() == "arty.rt.task.panicked"));

    let mut completed = tasks.add_test(async { 7 }, TaskOptions::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut completed), 7);
}

#[test]
fn self_wake_requests_an_immediate_follow_up_cycle() {
    let mut executor = Executor::builder().build();
    let polls = Rc::new(Cell::new(0));
    let mut completion = executor.tasks().add_test(
        poll_fn({
            let polls = Rc::clone(&polls);
            move |cx| {
                let poll = polls.get();
                polls.set(poll + 1);
                if poll < 2 {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(42)
                }
            }
        }),
        TaskOptions::default(),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Continue));
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Continue));
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut completion), 42);
}

struct PinnedDropFuture {
    polled_at: Cell<*const Self>,
    dropped: Rc<Cell<bool>>,
    completes: bool,
    _pin: PhantomPinned,
}

impl Future for PinnedDropFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_ref().get_ref();
        this.polled_at.set(std::ptr::from_ref(this));
        if this.completes { Poll::Ready(()) } else { Poll::Pending }
    }
}

impl Drop for PinnedDropFuture {
    fn drop(&mut self) {
        assert_eq!(self.polled_at.get(), std::ptr::from_ref(self));
        assert!(!self.dropped.replace(true));
    }
}

fn assert_future_is_destroyed_at_its_pinned_address(completes: bool) {
    let mut executor = Executor::builder().build();
    let dropped = Rc::new(Cell::new(false));
    let mut completion = executor.tasks().add_test(
        PinnedDropFuture {
            polled_at: Cell::new(std::ptr::null()),
            dropped: Rc::clone(&dropped),
            completes,
            _pin: PhantomPinned,
        },
        TaskOptions::default(),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    if completes {
        poll_completed(&mut completion);
    } else {
        executor.shutdown().trigger(ShutdownCause::default());
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
        poll_cancelled(&mut completion);
    }

    assert!(dropped.get());
}

#[test]
fn completed_future_is_destroyed_at_its_pinned_address() {
    assert_future_is_destroyed_at_its_pinned_address(true);
}

#[test]
fn cancelled_future_is_destroyed_at_its_pinned_address() {
    assert_future_is_destroyed_at_its_pinned_address(false);
}

#[test]
fn inactive_task_is_not_polled_again_without_a_wake() {
    let mut executor = Executor::builder().build();
    let polls = Rc::new(Cell::new(0));
    let _completion = executor.tasks().add_test(
        poll_fn({
            let polls = Rc::clone(&polls);
            move |_| {
                polls.set(polls.get() + 1);
                Poll::<()>::Pending
            }
        }),
        TaskOptions::default(),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(polls.get(), 1);
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(polls.get(), 1);
}

#[test]
fn tasks_submitted_during_poll_wait_for_the_next_cycle_in_registration_order() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let polls = Rc::new(RefCell::new(Vec::new()));

    for index in 0..2 {
        let child_tasks = tasks.clone();
        let polls = Rc::clone(&polls);
        tasks.add(
            async move {
                polls.borrow_mut().push(index);
                child_tasks.add(
                    {
                        let polls = Rc::clone(&polls);
                        async move {
                            polls.borrow_mut().push(index + 2);
                        }
                    },
                    TaskOptions::default(),
                    crate::support::DiscardCompletion,
                );
            },
            TaskOptions::default(),
            crate::support::DiscardCompletion,
        );
    }

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Continue));
    assert_eq!(*polls.borrow(), [0, 1]);
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(*polls.borrow(), [0, 1, 2, 3]);
}

#[test]
fn newly_submitted_tasks_run_before_awakened_tasks() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let polls = Rc::new(RefCell::new(Vec::new()));
    let ready = Rc::new(Cell::new(false));
    let retained = Rc::new(RefCell::new(None));
    tasks.add(
        poll_fn({
            let polls = Rc::clone(&polls);
            let ready = Rc::clone(&ready);
            let retained = Rc::clone(&retained);
            move |cx| {
                polls.borrow_mut().push(2);
                if ready.get() {
                    Poll::Ready(())
                } else {
                    retained.borrow_mut().replace(cx.waker().clone());
                    Poll::Pending
                }
            }
        }),
        TaskOptions::default(),
        crate::support::DiscardCompletion,
    );
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    polls.borrow_mut().clear();
    ready.set(true);
    retained.borrow().as_ref().unwrap().wake_by_ref();
    for index in 0..2 {
        tasks.add(
            {
                let polls = Rc::clone(&polls);
                async move {
                    polls.borrow_mut().push(index);
                }
            },
            TaskOptions::default(),
            crate::support::DiscardCompletion,
        );
    }

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(*polls.borrow(), [0, 1, 2]);
}

#[test]
fn shutdown_panic_policy_publishes_the_payload_before_shutdown() {
    let mut executor = Executor::builder().panic_policy(PanicPolicy::Shutdown).build();
    let mut completion = executor
        .tasks()
        .add_test(poll_fn(|_| -> Poll<()> { panic!("shutdown panic") }), TaskOptions::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut completion) else {
        panic!("panicking task must publish its payload before shutdown");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"shutdown panic"));
}

#[derive(Debug)]
struct PanicNotification {
    cause: Option<ShutdownCause>,
    executor_cause: Option<ShutdownCause>,
    original_payload: bool,
}

#[derive(Debug)]
struct CapturePanic {
    shutdown: Shutdown,
    notification: Rc<RefCell<Option<PanicNotification>>>,
}

impl TaskCompletion<()> for CapturePanic {
    fn completed(self, _result: ()) {}

    fn panicked(self, panic: TaskPanicPayload, cause: Option<ShutdownCause>) {
        let executor_cause = self.shutdown.is_triggered().then(|| self.shutdown.cause());
        if cause.is_some() {
            self.shutdown.trigger(ShutdownCause::default());
        }
        self.notification.replace(Some(PanicNotification {
            cause,
            executor_cause,
            original_payload: panic.downcast_ref::<&str>() == Some(&"panic notification"),
        }));
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[test]
fn panic_notification_carries_the_authoritative_shutdown_cause() {
    fn assert_same_cause(actual: &ShutdownCause, expected: &ShutdownCause) {
        match (actual.as_error(), expected.as_error()) {
            (Some(actual), Some(expected)) => assert!(std::ptr::addr_eq(actual.inner(), expected.inner())),
            (None, None) => {}
            _ => panic!("shutdown must preserve its first cause"),
        }
    }

    for (policy, existing) in [
        (PanicPolicy::Ignore, None),
        (PanicPolicy::Shutdown, None),
        (PanicPolicy::Ignore, Some(ShutdownCause::default())),
        (
            PanicPolicy::Shutdown,
            Some(ShutdownError::new(std::io::Error::other("first shutdown cause")).into()),
        ),
    ] {
        let expects_shutdown = matches!(policy, PanicPolicy::Shutdown) || existing.is_some();
        let mut executor = Executor::builder().panic_policy(policy).build();
        let shutdown = executor.shutdown();
        let notification = Rc::new(RefCell::new(None));
        executor.tasks().add(
            poll_fn({
                let shutdown = shutdown.clone();
                let mut existing = existing.clone();
                move |_| -> Poll<()> {
                    if let Some(cause) = existing.take() {
                        shutdown.trigger(cause);
                    }
                    panic!("panic notification");
                }
            }),
            TaskOptions::default(),
            CapturePanic {
                shutdown,
                notification: Rc::clone(&notification),
            },
        );

        let outcome = executor.execute_cycle();
        let notification = notification.take().unwrap();
        assert!(notification.original_payload);
        assert_eq!(notification.cause.is_some(), expects_shutdown);
        assert_eq!(notification.executor_cause.is_some(), expects_shutdown);
        if expects_shutdown {
            let cause = notification.cause.unwrap();
            assert_same_cause(&notification.executor_cause.unwrap(), &cause);
            let CycleOutcome::Shutdown(final_cause) = outcome else {
                panic!("a published shutdown cause must end the executor cycle");
            };
            assert_same_cause(&final_cause, &cause);
            if let Some(existing) = existing {
                assert_same_cause(&cause, &existing);
            } else {
                assert_eq!(
                    cause.as_error().unwrap().inner().to_string(),
                    "a contained runtime operation panicked"
                );
            }
        } else {
            assert!(matches!(outcome, CycleOutcome::Suspend));
        }
    }
}

struct ReadyDropPanic;

impl Future for ReadyDropPanic {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(42)
    }
}

impl Drop for ReadyDropPanic {
    #[expect(clippy::panic, reason = "this test injects a completed-future destructor panic")]
    fn drop(&mut self) {
        panic!("ready future destructor");
    }
}

#[test]
fn completed_future_destructor_panic_is_published_as_the_task_panic() {
    let mut executor = Executor::builder().panic_policy(PanicPolicy::Ignore).build();
    let mut completion = executor.tasks().add_test(ReadyDropPanic, TaskOptions::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut completion) else {
        panic!("completed future destructor panic must be published");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"ready future destructor"));
}

struct PollAndDropPanic;

impl Future for PollAndDropPanic {
    type Output = ();

    #[expect(clippy::panic, reason = "this test injects a task poll panic")]
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        panic!("poll panic");
    }
}

impl Drop for PollAndDropPanic {
    #[expect(clippy::panic, reason = "this test injects a secondary destructor panic")]
    fn drop(&mut self) {
        panic!("secondary destructor panic");
    }
}

#[test]
fn poll_panic_remains_primary_when_future_disposal_also_panics() {
    let mut executor = Executor::builder().panic_policy(PanicPolicy::Ignore).build();
    let mut completion = executor.tasks().add_test(PollAndDropPanic, TaskOptions::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut completion) else {
        panic!("poll panic must be published");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"poll panic"));
}

struct CancellationDropPanic {
    dropped: Rc<Cell<bool>>,
}

impl Future for CancellationDropPanic {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for CancellationDropPanic {
    #[expect(clippy::panic, reason = "this test injects a cancellation destructor panic")]
    fn drop(&mut self) {
        assert!(!self.dropped.replace(true));
        panic!("cancellation destructor");
    }
}

#[test]
fn cancellation_destructor_panic_still_publishes_cancellation_and_completes_shutdown() {
    let mut executor = Executor::builder().build();
    let dropped = Rc::new(Cell::new(false));
    let mut completion = executor.tasks().add_test(
        CancellationDropPanic {
            dropped: Rc::clone(&dropped),
        },
        TaskOptions::default(),
    );
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));

    executor.shutdown().trigger(ShutdownCause::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    assert!(dropped.get());
    assert!(matches!(poll_once(&mut completion), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn retained_handle_reports_shutdown_without_delaying_shutdown() {
    let mut executor = Executor::builder().build();
    let mut handle = executor.tasks().add_test(pending::<()>(), TaskOptions::default());

    executor.shutdown().trigger(ShutdownCause::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    assert!(!executor.has_tasks());
    drop(executor);
    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn caused_shutdown_is_attached_to_cancelled_and_rejected_tasks() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let mut pending = tasks.add_test(pending::<()>(), TaskOptions::default());
    let (cause, expected) = shutdown_cause("executor failed");

    executor.shutdown().trigger(cause);
    let mut rejected = tasks.add_test(async {}, TaskOptions::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));

    for cause in [poll_cancelled(&mut pending), poll_cancelled(&mut rejected)] {
        let source = cause.as_error().unwrap();
        assert_same_shutdown_source(source, &expected);
    }
}

#[test]
fn clean_shutdown_remains_authoritative_for_rejected_tasks() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let cause = shutdown_cause("later failure").0;

    executor.shutdown().trigger(ShutdownCause::default());
    let mut rejected = tasks.add_test(async {}, TaskOptions::default());
    executor.shutdown().trigger(cause);

    assert!(poll_cancelled(&mut rejected).as_error().is_none());
}

#[test]
fn retained_task_set_preserves_the_shutdown_cause_after_executor_drop() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let (cause, expected) = shutdown_cause("owner failed");
    executor.shutdown().trigger(cause);
    drop(executor);

    let mut local = tasks.add_test(async {}, TaskOptions::default());
    let cause = poll_cancelled(&mut local);
    let source = cause.as_error().unwrap();
    assert_same_shutdown_source(source, &expected);
}

#[test]
fn direct_executor_drop_cancels_tasks_safely() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add_test(pending::<()>(), TaskOptions::default());

    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn shutdown_handle_cancels_tasks_without_executor_drop() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let shutdown = executor.shutdown();
    let mut handle = tasks.add_test(pending::<()>(), TaskOptions::default());

    shutdown.trigger(ShutdownCause::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    assert!(!executor.has_tasks());
    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn executor_drop_during_thread_local_teardown_is_safe() {
    std::thread::spawn(|| {
        THREAD_LOCAL_EXECUTOR.with(|slot| {
            assert!(slot.replace(Some(Executor::builder().build())).is_none());
        });
        THREAD_LOCAL_EXECUTOR.with(|slot| {
            let mut executor = slot.borrow_mut();
            assert!(matches!(
                executor.as_mut().expect("thread-local executor is initialized").execute_cycle(),
                CycleOutcome::Suspend
            ));
        });
    })
    .join()
    .unwrap();
}

#[test]
fn disconnected_task_set_returns_shutdown_error() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    drop(executor);

    let mut handle = tasks.add_test(async { 42 }, TaskOptions::default());

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

struct IntoFutureProbe(Arc<AtomicBool>);

impl IntoFuture for IntoFutureProbe {
    type Output = ();
    type IntoFuture = Ready<()>;

    fn into_future(self) -> Self::IntoFuture {
        self.0.store(true, Ordering::Release);
        ready(())
    }
}

#[test]
fn rejected_submission_does_not_convert_the_future() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.shutdown().trigger(ShutdownCause::default());
    let converted = Arc::new(AtomicBool::new(false));

    let mut handle = tasks.add_test(IntoFutureProbe(Arc::clone(&converted)), TaskOptions::default());

    assert!(!converted.load(Ordering::Acquire));
    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn submission_during_shutdown_returns_shutdown_error() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.shutdown().trigger(ShutdownCause::default());

    let mut handle = tasks.add_test(async { 42 }, TaskOptions::default());

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn task_created_processor_may_submit_a_nested_task() {
    let sink = Sink::builder("arty-executor-reentrant-telemetry")
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(ReentrantSubmissionProcessor)])
        .build();
    let mut executor = Executor::builder().sink(sink.clone()).build();
    let tasks = executor.tasks();
    let nested_completed = Rc::new(Cell::new(false));
    REENTRANT_TELEMETRY_SUBMISSION.set(Some((tasks.clone(), Rc::clone(&nested_completed))));

    let _outer = tasks.add_test(async {}, TaskOptions::observed(sink.transfer_context(), TaskPlacement::Any));

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert!(nested_completed.get());
}

#[test]
fn default_options_emit_lifecycle_events_without_context_enrichment() {
    let (sink, processor) = test_emitter(TEST_ID);
    let mut executor = Executor::builder().sink(sink).build();
    let _handle = executor.tasks().add_test(async {}, TaskOptions::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));

    let events = processor.events();
    assert!(events.iter().any(|event| event.name() == "arty.rt.task.spawned"));
    assert!(events.iter().any(|event| event.name() == "arty.rt.task.succeeded"));
}

struct ReentrantIntoFuture {
    tasks: TaskSet,
    nested_completed: Rc<Cell<bool>>,
}

impl IntoFuture for ReentrantIntoFuture {
    type Output = usize;
    type IntoFuture = std::future::Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.tasks.add_test(
            {
                let nested_completed = Rc::clone(&self.nested_completed);
                async move {
                    nested_completed.set(true);
                }
            },
            TaskOptions::default(),
        );
        std::future::ready(42)
    }
}

#[test]
fn into_future_conversion_may_submit_a_nested_task() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let nested_completed = Rc::new(Cell::new(false));
    let mut handle = tasks.add_test(
        ReentrantIntoFuture {
            tasks: tasks.clone(),
            nested_completed: Rc::clone(&nested_completed),
        },
        TaskOptions::default(),
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut handle), 42);
    assert!(nested_completed.get());
}

struct DropOwnerDuringIntoFuture {
    owner: Rc<RefCell<Option<Executor>>>,
    tasks: TaskSet,
    nested: Rc<RefCell<Option<CompletionHandle<usize>>>>,
}

impl IntoFuture for DropOwnerDuringIntoFuture {
    type Output = usize;
    type IntoFuture = Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        drop(self.owner.borrow_mut().take());
        let nested = self.tasks.add_test(async { 42 }, TaskOptions::default());
        assert!(self.nested.replace(Some(nested)).is_none());
        ready(7)
    }
}

#[test]
fn owner_drop_during_conversion_cancels_tasks_and_rejects_nested_submission() {
    let owner = Rc::new(RefCell::new(Some(Executor::builder().build())));
    let tasks = owner.borrow().as_ref().unwrap().tasks();
    let mut existing = tasks.add_test(pending::<()>(), TaskOptions::default());
    let nested = Rc::new(RefCell::new(None));

    let mut outer = tasks.add_test(
        DropOwnerDuringIntoFuture {
            owner: Rc::clone(&owner),
            tasks: tasks.clone(),
            nested: Rc::clone(&nested),
        },
        TaskOptions::default(),
    );

    assert!(owner.borrow().is_none());
    assert!(matches!(poll_once(&mut existing), Poll::Ready(TaskOutcome::Cancelled(_))));
    assert!(matches!(poll_once(&mut outer), Poll::Ready(TaskOutcome::Cancelled(_))));
    let mut nested = nested.borrow_mut().take().unwrap();
    assert!(matches!(poll_once(&mut nested), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn owner_drop_during_spawn_telemetry_cancels_task_and_rejects_nested_submission() {
    let sink = Sink::builder("arty-executor-owner-drop")
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(OwnerDropProcessor)])
        .build();
    let executor = Executor::builder().sink(sink).build();
    let tasks = executor.tasks();
    let nested = Rc::new(RefCell::new(None));
    OWNER_DROP_TELEMETRY.set(Some(OwnerDropState {
        owner: executor,
        tasks: tasks.clone(),
        nested: Rc::clone(&nested),
    }));

    let mut outer = tasks.add_test(pending::<()>(), TaskOptions::default());

    assert!(matches!(poll_once(&mut outer), Poll::Ready(TaskOutcome::Cancelled(_))));
    let mut nested = nested.borrow_mut().take().unwrap();
    assert!(matches!(poll_once(&mut nested), Poll::Ready(TaskOutcome::Cancelled(_))));
}

struct ShutdownDuringIntoFuture {
    shutdown: Shutdown,
}

impl IntoFuture for ShutdownDuringIntoFuture {
    type Output = usize;
    type IntoFuture = std::future::Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.shutdown.trigger(ShutdownCause::default());
        std::future::ready(42)
    }
}

#[test]
fn shutdown_during_into_future_conversion_rejects_the_outer_task() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let mut handle = tasks.add_test(
        ShutdownDuringIntoFuture {
            shutdown: executor.shutdown(),
        },
        TaskOptions::default(),
    );

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
}

struct SubmitOnDrop {
    tasks: TaskSet,
    submitted: Rc<RefCell<Option<CompletionHandle<usize>>>>,
}

impl Future for SubmitOnDrop {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for SubmitOnDrop {
    fn drop(&mut self) {
        let handle = self.tasks.add_test(async { 42 }, TaskOptions::default());
        assert!(self.submitted.replace(Some(handle)).is_none());
    }
}

#[test]
fn cancelled_future_drop_can_submit_and_receive_shutdown_error() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let submitted = Rc::new(RefCell::new(None));
    let _outer_handle = tasks.add_test(
        SubmitOnDrop {
            tasks: tasks.clone(),
            submitted: Rc::clone(&submitted),
        },
        TaskOptions::default(),
    );
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));

    executor.shutdown().trigger(ShutdownCause::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    let mut submitted_handle = submitted.borrow_mut().take().expect("future destructor submitted a task");
    assert!(matches!(poll_once(&mut submitted_handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn shutdown_retires_a_batch_of_queued_tasks() {
    const TASK_COUNT: usize = 32;

    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let wakers = Rc::new(RefCell::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..TASK_COUNT {
        handles.push(tasks.add_test(
            {
                let wakers = Rc::clone(&wakers);
                poll_fn(move |cx| {
                    wakers.borrow_mut().push(cx.waker().clone());
                    Poll::<()>::Pending
                })
            },
            TaskOptions::default(),
        ));
    }
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    for waker in wakers.borrow().iter() {
        waker.wake_by_ref();
    }

    executor.shutdown().trigger(ShutdownCause::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    for mut handle in handles {
        assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
    }
}
