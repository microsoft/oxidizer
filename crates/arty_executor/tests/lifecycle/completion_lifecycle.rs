// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public task-completion and executor-lifecycle contracts.

use std::cell::{Cell, RefCell};
use std::error::Error as _;
use std::fmt;
use std::future::{Ready, pending, poll_fn, ready};
use std::marker::PhantomPinned;
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use arty_executor::{
    CycleOutcome, Executor, PanicAction, PanicPolicy, Shutdown, ShutdownCause, ShutdownError, TaskCompletion, TaskInstrumentation,
    TaskPanicPayload, TaskSet,
};

use super::support::{CompletionHandle, TaskBuilderExt as _, TaskOutcome};

thread_local! {
    static THREAD_LOCAL_EXECUTOR: RefCell<Option<Executor>> = const { RefCell::new(None) };
}

#[test]
fn task_set_execution_state_follows_the_owner_cycle() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let observed = Rc::new(Cell::new(false));
    let task_observed = Rc::clone(&observed);
    let task_tasks = tasks.clone();
    assert!(!tasks.is_executing());
    let _completion = tasks
        .task(async move {
            task_observed.set(task_tasks.is_executing());
        })
        .register_test();

    _ = executor.execute_cycle();

    assert!(observed.get());
    assert!(!tasks.is_executing());
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
    assert!(!actual.is_panic());
    let source = actual.source().unwrap().downcast_ref::<TestShutdownError>().unwrap();
    let expected = expected.inner().downcast_ref::<TestShutdownError>().unwrap();
    assert!(std::ptr::eq(source, expected));
    assert_eq!(format!("{cause:?}"), "ShutdownCause { has_error: true }");
}

#[test]
fn panic_causes_retain_messages_without_consuming_payloads() {
    let payloads: [(TaskPanicPayload, &str); 3] = [
        (Box::new("borrowed panic"), "borrowed panic"),
        (Box::new(String::from("owned panic")), "owned panic"),
        (Box::new(42_u32), "non-string panic payload"),
    ];

    for (payload, message) in payloads {
        let cause = ShutdownCause::panic(payload.as_ref());
        let error = cause.as_error().unwrap();
        let cloned = cause.clone().into_error().unwrap();

        assert!(error.is_panic());
        assert!(cloned.is_panic());
        assert_eq!(error.to_string(), format!("runtime shutdown was caused by a task panic: {message}"));
        assert!(error.inner().source().is_none());
        assert!(std::ptr::addr_eq(error.inner(), cloned.inner()));
        if let Some(value) = payload.downcast_ref::<u32>() {
            assert_eq!(*value, 42);
        } else if let Some(value) = payload.downcast_ref::<String>() {
            assert_eq!(value, message);
        } else {
            assert_eq!(*payload.downcast::<&str>().unwrap(), message);
        }
    }

    let same_message = ShutdownError::new(TestShutdownError("runtime shutdown was caused by a task panic: borrowed panic"));
    assert!(!same_message.is_panic());
    assert!(ShutdownCause::panic(&()).as_error().unwrap().is_panic());
    assert!(ShutdownCause::default().as_error().is_none());
}

#[test]
fn failed_construction_preserves_existing_executor_notifications() {
    let mut executor = Executor::builder().build();
    let ready = Rc::new(Cell::new(false));
    let retained = Rc::new(RefCell::new(None));
    let mut handle = executor
        .tasks()
        .task(poll_fn({
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
        }))
        .register_test();
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    ready.set(true);
    retained.borrow().as_ref().unwrap().wake_by_ref();

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Executor::builder().build())).unwrap_err();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut handle), 42);
}

#[test]
fn configured_current_thread_executes_tasks() {
    let thread = thread_aware_core::__private::v1::new_thread(
        thread_aware_core::__private::v1::new_owner(),
        std::thread::current().id(),
        thread_aware_core::__private::v1::new_numa_node(0),
    );
    let mut executor = Executor::builder().thread(thread).build();
    let mut handle = executor.tasks().task(async { 42 }).register_test();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut handle), 42);
    assert!(!executor.has_tasks());
}

#[test]
fn completed_result_remains_available_after_executor_stops() {
    let mut executor = Executor::builder().build();
    let mut handle = executor.tasks().task(async { 42 }).register_test();

    assert!(executor.has_tasks());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert!(!executor.has_tasks());
    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Completed(42))));
}

#[test]
fn task_panic_publishes_the_original_payload_and_keeps_the_executor_usable() {
    let mut executor = Executor::builder().panic_policy(PanicPolicy::IGNORE).build();
    let tasks = executor.tasks();
    let mut panicked = tasks.task(poll_fn(|_| -> Poll<usize> { panic!("task panic") })).register_test();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut panicked) else {
        panic!("panicking task must publish its panic payload");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"task panic"));

    let mut completed = tasks.task(async { 7 }).register_test();
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(poll_completed(&mut completed), 7);
}

#[test]
fn self_wake_requests_an_immediate_follow_up_cycle() {
    let mut executor = Executor::builder().build();
    let polls = Rc::new(Cell::new(0));
    let mut completion = executor
        .tasks()
        .task(poll_fn({
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
        }))
        .register_test();

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
    let mut completion = executor
        .tasks()
        .task(PinnedDropFuture {
            polled_at: Cell::new(std::ptr::null()),
            dropped: Rc::clone(&dropped),
            completes,
            _pin: PhantomPinned,
        })
        .register_test();

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
    let _completion = executor
        .tasks()
        .task(poll_fn({
            let polls = Rc::clone(&polls);
            move |_| {
                polls.set(polls.get() + 1);
                Poll::<()>::Pending
            }
        }))
        .register_test();

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
        tasks
            .task(async move {
                polls.borrow_mut().push(index);
                child_tasks
                    .task({
                        let polls = Rc::clone(&polls);
                        async move {
                            polls.borrow_mut().push(index + 2);
                        }
                    })
                    .register(super::support::DiscardCompletion);
            })
            .register(super::support::DiscardCompletion);
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
    tasks
        .task(poll_fn({
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
        }))
        .register(super::support::DiscardCompletion);
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    polls.borrow_mut().clear();
    ready.set(true);
    retained.borrow().as_ref().unwrap().wake_by_ref();
    for index in 0..2 {
        tasks
            .task({
                let polls = Rc::clone(&polls);
                async move {
                    polls.borrow_mut().push(index);
                }
            })
            .register(super::support::DiscardCompletion);
    }

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert_eq!(*polls.borrow(), [0, 1, 2]);
}

#[test]
fn shutdown_panic_policy_publishes_the_payload_before_shutdown() {
    let mut executor = Executor::builder().panic_policy(PanicPolicy::SHUTDOWN).build();
    let mut completion = executor
        .tasks()
        .task(poll_fn(|_| -> Poll<()> { panic!("shutdown panic") }))
        .register_test();

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
    fn shutdown(panic: TaskPanicPayload) -> (PanicAction, TaskPanicPayload) {
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"panic notification"));
        (PanicAction::Shutdown, panic)
    }

    fn assert_same_cause(actual: &ShutdownCause, expected: &ShutdownCause) {
        match (actual.as_error(), expected.as_error()) {
            (Some(actual), Some(expected)) => assert!(std::ptr::addr_eq(actual.inner(), expected.inner())),
            (None, None) => {}
            _ => panic!("shutdown must preserve its first cause"),
        }
    }

    let inspected = Rc::new(Cell::new(false));
    let dynamic_ignore = PanicPolicy::dynamic({
        let inspected = Rc::clone(&inspected);
        move |panic| {
            assert_eq!(panic.downcast_ref::<&str>(), Some(&"panic notification"));
            inspected.set(true);
            (PanicAction::Ignore, panic)
        }
    });

    for (policy, existing, expects_policy_shutdown) in [
        (PanicPolicy::IGNORE, None, false),
        (PanicPolicy::SHUTDOWN, None, true),
        (dynamic_ignore, None, false),
        (PanicPolicy::dynamic(shutdown), None, true),
        (PanicPolicy::IGNORE, Some(ShutdownCause::default()), false),
        (
            PanicPolicy::SHUTDOWN,
            Some(ShutdownError::new(std::io::Error::other("first shutdown cause")).into()),
            true,
        ),
    ] {
        let expects_shutdown = expects_policy_shutdown || existing.is_some();
        let mut executor = Executor::builder().panic_policy(policy).build();
        let shutdown = executor.shutdown();
        let notification = Rc::new(RefCell::new(None));
        executor
            .tasks()
            .task(poll_fn({
                let shutdown = shutdown.clone();
                let mut existing = existing.clone();
                move |_| -> Poll<()> {
                    if let Some(cause) = existing.take() {
                        shutdown.trigger(cause);
                    }
                    panic!("panic notification");
                }
            }))
            .register(CapturePanic {
                shutdown,
                notification: Rc::clone(&notification),
            });

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
                    "runtime shutdown was caused by a task panic: panic notification"
                );
            }
        } else {
            assert!(matches!(outcome, CycleOutcome::Suspend));
        }
    }
    assert!(inspected.get());
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
    let mut executor = Executor::builder().panic_policy(PanicPolicy::IGNORE).build();
    let mut completion = executor.tasks().task(ReadyDropPanic).register_test();

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
    let mut executor = Executor::builder().panic_policy(PanicPolicy::IGNORE).build();
    let mut completion = executor.tasks().task(PollAndDropPanic).register_test();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut completion) else {
        panic!("poll panic must be published");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"poll panic"));
}

struct CancellationDropPanic {
    dropped: Rc<Cell<bool>>,
}

struct PanicOnce(AtomicBool);

impl Drop for PanicOnce {
    fn drop(&mut self) {
        assert!(self.0.swap(true, Ordering::Relaxed), "cancellation panic payload destructor");
    }
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
        std::panic::panic_any(PanicOnce(AtomicBool::new(false)));
    }
}

#[test]
fn cancellation_destructor_panic_still_publishes_cancellation_and_completes_shutdown() {
    let mut executor = Executor::builder().build();
    let dropped = Rc::new(Cell::new(false));
    let mut completion = executor
        .tasks()
        .task(CancellationDropPanic {
            dropped: Rc::clone(&dropped),
        })
        .register_test();
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));

    executor.shutdown().trigger(ShutdownCause::default());

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
    assert!(dropped.get());
    assert!(matches!(poll_once(&mut completion), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn retained_handle_reports_shutdown_without_delaying_shutdown() {
    let mut executor = Executor::builder().build();
    let mut handle = executor.tasks().task(pending::<()>()).register_test();

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
    let mut pending = tasks.task(pending::<()>()).register_test();
    let (cause, expected) = shutdown_cause("executor failed");

    executor.shutdown().trigger(cause);
    let mut rejected = tasks.task(async {}).register_test();
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
    let mut rejected = tasks.task(async {}).register_test();
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

    let mut local = tasks.task(async {}).register_test();
    let cause = poll_cancelled(&mut local);
    let source = cause.as_error().unwrap();
    assert_same_shutdown_source(source, &expected);
}

#[test]
fn direct_executor_drop_cancels_tasks_safely() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().task(pending::<()>()).register_test();

    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn shutdown_handle_cancels_tasks_without_executor_drop() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let shutdown = executor.shutdown();
    let mut handle = tasks.task(pending::<()>()).register_test();

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

    let mut handle = tasks.task(async { 42 }).register_test();

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
fn task_builder_defers_conversion_and_registration() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let converted = Arc::new(AtomicBool::new(false));
    let builder = tasks.task(IntoFutureProbe(Arc::clone(&converted)));

    assert!(!converted.load(Ordering::Acquire));
    assert!(!executor.has_tasks());
    drop(builder);
    assert!(!converted.load(Ordering::Acquire));
    assert!(!executor.has_tasks());

    let builder = tasks.task(IntoFutureProbe(Arc::clone(&converted)));
    let mut handle = builder.register_test();

    assert!(converted.load(Ordering::Acquire));
    assert!(executor.has_tasks());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Completed(()))));
}

#[test]
fn rejected_submission_does_not_convert_the_future() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.shutdown().trigger(ShutdownCause::default());
    let converted = Arc::new(AtomicBool::new(false));

    let mut handle = tasks.task(IntoFutureProbe(Arc::clone(&converted))).register_test();

    assert!(!converted.load(Ordering::Acquire));
    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[test]
fn submission_during_shutdown_returns_shutdown_error() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.shutdown().trigger(ShutdownCause::default());

    let mut handle = tasks.task(async { 42 }).register_test();

    assert!(matches!(poll_once(&mut handle), Poll::Ready(TaskOutcome::Cancelled(_))));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstrumentationEvent {
    PollStart,
    PollEnd(Poll<()>),
    Completed,
    Panicked,
    Cancelled,
}

#[derive(Clone)]
struct RecordingInstrumentation(Rc<RefCell<Vec<InstrumentationEvent>>>);

impl TaskInstrumentation for RecordingInstrumentation {
    fn poll_start(&mut self) {
        self.0.borrow_mut().push(InstrumentationEvent::PollStart);
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.0.borrow_mut().push(InstrumentationEvent::PollEnd(outcome));
    }

    fn completed(&mut self) {
        self.0.borrow_mut().push(InstrumentationEvent::Completed);
    }

    fn panicked(&mut self) {
        self.0.borrow_mut().push(InstrumentationEvent::Panicked);
    }

    fn cancelled(&mut self) {
        self.0.borrow_mut().push(InstrumentationEvent::Cancelled);
    }
}

#[test]
fn task_instrumentation_receives_each_poll_and_terminal_event() {
    let mut executor = Executor::builder().build();
    let events = Rc::new(RefCell::new(Vec::new()));
    let ready = Rc::new(Cell::new(false));
    let retained = Rc::new(RefCell::new(None));
    let mut completion = executor
        .tasks()
        .task(poll_fn({
            let ready = Rc::clone(&ready);
            let retained = Rc::clone(&retained);
            move |cx| {
                if ready.get() {
                    Poll::Ready(())
                } else {
                    retained.borrow_mut().replace(cx.waker().clone());
                    Poll::Pending
                }
            }
        }))
        .instrument(RecordingInstrumentation(Rc::clone(&events)))
        .inspect(|_| Some(()))
        .register_test();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    ready.set(true);
    retained.borrow().as_ref().unwrap().wake_by_ref();
    let _ = executor.execute_cycle();
    assert!(matches!(poll_once(&mut completion), Poll::Ready(TaskOutcome::Completed(()))));
    assert_eq!(
        *events.borrow(),
        [
            InstrumentationEvent::PollStart,
            InstrumentationEvent::PollEnd(Poll::Pending),
            InstrumentationEvent::PollStart,
            InstrumentationEvent::PollEnd(Poll::Ready(())),
            InstrumentationEvent::Completed,
        ]
    );
}

#[derive(Clone, Copy)]
enum PanickingCallback {
    Completed,
    Panicked,
    Cancelled,
}

struct PanicOn(PanickingCallback);

impl TaskInstrumentation for PanicOn {
    #[expect(clippy::panic, reason = "this test injects a completion callback panic")]
    fn completed(&mut self) {
        if matches!(self.0, PanickingCallback::Completed) {
            panic!("completion callback");
        }
    }

    #[expect(clippy::panic, reason = "this test injects a panic callback panic")]
    fn panicked(&mut self) {
        if matches!(self.0, PanickingCallback::Panicked) {
            panic!("panic callback");
        }
    }

    #[expect(clippy::panic, reason = "this test injects a cancellation callback panic")]
    fn cancelled(&mut self) {
        if matches!(self.0, PanickingCallback::Cancelled) {
            panic!("cancellation callback");
        }
    }
}

#[derive(Debug)]
struct ObserveCancellation(Rc<Cell<bool>>);

impl TaskCompletion<()> for ObserveCancellation {
    fn completed(self, _result: ()) {}

    fn panicked(self, _panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {}

    fn cancelled(self, _cause: ShutdownCause) {
        self.0.set(true);
    }
}

#[test]
fn instrumentation_panics_do_not_prevent_task_completion_notifications() {
    let mut executor = Executor::builder().build();
    let mut completed = executor
        .tasks()
        .task(async {})
        .instrument(PanicOn(PanickingCallback::Completed))
        .inspect(|_| Some(()))
        .register_test();
    let _panic = catch_unwind(AssertUnwindSafe(|| executor.execute_cycle())).unwrap_err();
    assert!(matches!(poll_once(&mut completed), Poll::Ready(TaskOutcome::Completed(()))));
    drop(executor);

    let mut executor = Executor::builder().build();
    let mut panicked = executor
        .tasks()
        .task(poll_fn(|_| -> Poll<()> { panic!("task panic") }))
        .instrument(PanicOn(PanickingCallback::Panicked))
        .inspect(|_| Some(()))
        .register_test();
    let _panic = catch_unwind(AssertUnwindSafe(|| executor.execute_cycle())).unwrap_err();
    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut panicked) else {
        panic!("panicking task must publish its payload");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"task panic"));
    drop(executor);

    let executor = Executor::builder().build();
    executor.shutdown().trigger(ShutdownCause::default());
    let cancelled = Rc::new(Cell::new(false));
    let result = catch_unwind(AssertUnwindSafe(|| {
        executor
            .tasks()
            .task(async {})
            .instrument(PanicOn(PanickingCallback::Cancelled))
            .inspect(|_| Some(()))
            .register(ObserveCancellation(Rc::clone(&cancelled)));
    }));
    let _panic = result.unwrap_err();
    assert!(cancelled.get());
}

struct PanickingPayloadDrop(Arc<AtomicUsize>);

impl Drop for PanickingPayloadDrop {
    #[expect(clippy::panic, reason = "this test injects a panic-payload destructor panic")]
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
        panic_any("panic payload destructor");
    }
}

struct PollAndDropPanicPayload(Arc<AtomicUsize>);

impl Future for PollAndDropPanicPayload {
    type Output = ();

    #[expect(clippy::panic, reason = "this test injects a task poll panic")]
    fn poll(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        panic_any("primary task panic");
    }
}

impl Drop for PollAndDropPanicPayload {
    #[expect(clippy::panic, reason = "this test injects a secondary future destructor panic")]
    fn drop(&mut self) {
        panic_any(PanickingPayloadDrop(Arc::clone(&self.0)));
    }
}

#[test]
fn dropping_a_secondary_panic_payload_preserves_the_task_panic() {
    let mut executor = Executor::builder().build();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut completion = executor.tasks().task(PollAndDropPanicPayload(Arc::clone(&drops))).register_test();

    let _ = executor.execute_cycle();

    let Poll::Ready(TaskOutcome::Panicked(payload)) = poll_once(&mut completion) else {
        panic!("panicking task must publish its payload");
    };
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"primary task panic"));
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

struct ReentrantIntoFuture {
    tasks: TaskSet,
    nested_completed: Rc<Cell<bool>>,
}

impl IntoFuture for ReentrantIntoFuture {
    type Output = usize;
    type IntoFuture = std::future::Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.tasks
            .task({
                let nested_completed = Rc::clone(&self.nested_completed);
                async move {
                    nested_completed.set(true);
                }
            })
            .register_test();
        std::future::ready(42)
    }
}

#[test]
fn into_future_conversion_may_submit_a_nested_task() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let nested_completed = Rc::new(Cell::new(false));
    let mut handle = tasks
        .task(ReentrantIntoFuture {
            tasks: tasks.clone(),
            nested_completed: Rc::clone(&nested_completed),
        })
        .register_test();

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
        let nested = self.tasks.task(async { 42 }).register_test();
        assert!(self.nested.replace(Some(nested)).is_none());
        ready(7)
    }
}

#[test]
fn owner_drop_during_conversion_cancels_tasks_and_rejects_nested_submission() {
    let owner = Rc::new(RefCell::new(Some(Executor::builder().build())));
    let tasks = owner.borrow().as_ref().unwrap().tasks();
    let mut existing = tasks.task(pending::<()>()).register_test();
    let nested = Rc::new(RefCell::new(None));

    let mut outer = tasks
        .task(DropOwnerDuringIntoFuture {
            owner: Rc::clone(&owner),
            tasks: tasks.clone(),
            nested: Rc::clone(&nested),
        })
        .register_test();

    assert!(owner.borrow().is_none());
    assert!(matches!(poll_once(&mut existing), Poll::Ready(TaskOutcome::Cancelled(_))));
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
    let mut handle = tasks
        .task(ShutdownDuringIntoFuture {
            shutdown: executor.shutdown(),
        })
        .register_test();

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
        let handle = self.tasks.task(async { 42 }).register_test();
        assert!(self.submitted.replace(Some(handle)).is_none());
    }
}

#[test]
fn cancelled_future_drop_can_submit_and_receive_shutdown_error() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let submitted = Rc::new(RefCell::new(None));
    let _outer_handle = tasks
        .task(SubmitOnDrop {
            tasks: tasks.clone(),
            submitted: Rc::clone(&submitted),
        })
        .register_test();
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
        handles.push(
            tasks
                .task({
                    let wakers = Rc::clone(&wakers);
                    poll_fn(move |cx| {
                        wakers.borrow_mut().push(cx.waker().clone());
                        Poll::<()>::Pending
                    })
                })
                .register_test(),
        );
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
