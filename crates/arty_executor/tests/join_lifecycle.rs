// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public join-handle and executor-lifecycle contracts.

use std::cell::{Cell, RefCell};
use std::error::Error as _;
use std::fmt;
use std::future::{Ready, pending, poll_fn, ready};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use arty_executor::remote::Pool as RemotePool;
use arty_executor::{CycleOutcome, Executor, JoinError, JoinHandle, ShutdownCause, ShutdownError, TaskOptions, TaskPlacement, TaskSet};
use observed::Sink;
use observed::enrichment::{Enrichment, EnrichmentEntry};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed_testing::{TEST_ID, test_emitter};

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
    nested: Rc<RefCell<Option<JoinHandle<usize>>>>,
}

impl EventProcessor for ReentrantSubmissionProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == "arty.rt.task.spawned"
    }

    fn process(&self, _event: &EventView<'_>) {
        let Some((tasks, completed)) = REENTRANT_TELEMETRY_SUBMISSION.take() else {
            return;
        };
        drop(tasks.add(
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
        let nested = state.tasks.add(async { 42 }, TaskOptions::default());
        assert!(state.nested.replace(Some(nested)).is_none());
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

fn poll_once<R: 'static>(handle: &mut JoinHandle<R>) -> Poll<Result<R, JoinError>> {
    Pin::new(handle).poll(&mut Context::from_waker(Waker::noop()))
}

#[expect(clippy::panic, reason = "test helper reports an unexpected pending or successful result")]
fn ready_error<T, E>(poll: Poll<Result<T, E>>) -> E {
    let Poll::Ready(Err(error)) = poll else {
        panic!("join handle must be immediately ready with an error");
    };
    error
}

fn poll_error<R: 'static>(handle: &mut JoinHandle<R>) -> JoinError {
    ready_error(poll_once(handle))
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
    (ShutdownCause::from_error(error.clone()), error)
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
    let mut handle = executor.tasks().add(
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
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    ready.set(true);
    retained.borrow().as_ref().unwrap().wake_by_ref();

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Executor::builder().build())).unwrap_err();

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(42)));
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
    let mut handle = executor.tasks().add(
        async move { enrichment_keys(&task_sink) },
        TaskOptions::observed(task_context, TaskPlacement::Any),
    );

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(vec![String::from("task.context")])));
    assert_eq!(enrichment_keys(&sink), ["caller.context"]);
}

#[test]
fn completed_result_survives_executor_drop() {
    let mut executor = Executor::builder().build();
    let mut handle = executor.tasks().add(async { 42 }, TaskOptions::default());

    assert!(executor.has_tasks());
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert!(!executor.has_tasks());
    drop(executor);

    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(42)));
}

struct ShutdownDuringRemoteConversion(TaskSet);

impl IntoFuture for ShutdownDuringRemoteConversion {
    type Output = usize;
    type IntoFuture = Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.0.begin_shutdown(ShutdownCause::default());
        ready(42)
    }
}

#[test]
fn shutdown_during_remote_conversion_rejects_the_task() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let result_pool = RemotePool::new();
    let (result_tx, mut result) = result_pool.channel();

    tasks.add_with_remote_result(ShutdownDuringRemoteConversion(tasks.clone()), TaskOptions::default(), result_tx);

    assert!(!executor.has_tasks());
    assert!(matches!(
        Pin::new(&mut result).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(error)) if error.is_shutdown()
    ));
}

#[test]
fn retained_handle_reports_shutdown_without_delaying_shutdown() {
    let mut executor = Executor::builder().build();
    let mut handle = executor.tasks().add(pending::<()>(), TaskOptions::default());

    executor.begin_shutdown(ShutdownCause::default());

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    assert!(!executor.has_tasks());
    drop(executor);
    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn caused_shutdown_is_attached_to_cancelled_and_rejected_tasks() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let mut pending = tasks.add(pending::<()>(), TaskOptions::default());
    let (cause, expected) = shutdown_cause("executor failed");

    executor.begin_shutdown(cause);
    let mut rejected = tasks.add(async {}, TaskOptions::default());

    for error in [poll_error(&mut pending), poll_error(&mut rejected)] {
        let source = error.source().unwrap().downcast_ref::<ShutdownError>().unwrap();
        assert_same_shutdown_source(source, &expected);
    }
}

#[test]
fn remote_task_cancellation_carries_the_shutdown_cause() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let (result_tx, remote_join) = RemotePool::new().channel::<usize>();
    tasks.add_with_remote_result(pending::<usize>(), TaskOptions::default(), result_tx);
    let (cause, expected) = shutdown_cause("remote executor failed");

    executor.begin_shutdown(cause);
    let (rejected_tx, rejected_join) = RemotePool::new().channel::<usize>();
    tasks.add_with_remote_result(async { 42 }, TaskOptions::default(), rejected_tx);

    for mut join in [remote_join, rejected_join] {
        let error = ready_error(Pin::new(&mut join).poll(&mut Context::from_waker(Waker::noop())));
        let source = error.source().unwrap().downcast_ref::<ShutdownError>().unwrap();
        assert_same_shutdown_source(source, &expected);
    }
}

#[test]
fn first_shutdown_cause_remains_authoritative() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();

    executor.begin_shutdown(ShutdownCause::default());
    executor.begin_shutdown(shutdown_cause("later failure").0);
    let mut rejected = tasks.add(async {}, TaskOptions::default());

    assert!(poll_error(&mut rejected).source().is_none());
}

#[test]
fn retained_task_set_preserves_the_shutdown_cause_after_executor_drop() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let (cause, expected) = shutdown_cause("owner failed");
    executor.begin_shutdown(cause);
    drop(executor);

    let mut local = tasks.add(async {}, TaskOptions::default());
    let (remote_tx, remote) = RemotePool::new().channel::<usize>();
    tasks.add_with_remote_result(async { 42 }, TaskOptions::default(), remote_tx);

    let local = poll_error(&mut local);
    let mut remote = std::pin::pin!(remote);
    let remote = ready_error(remote.as_mut().poll(&mut Context::from_waker(Waker::noop())));
    for error in [local, remote] {
        let source = error.source().unwrap().downcast_ref::<ShutdownError>().unwrap();
        assert_same_shutdown_source(source, &expected);
    }
}

#[test]
fn direct_executor_drop_cancels_tasks_safely() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add(pending::<()>(), TaskOptions::default());

    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn task_set_shutdown_cancels_tasks_without_executor_drop() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let mut handle = tasks.add(pending::<()>(), TaskOptions::default());

    tasks.begin_shutdown(ShutdownCause::default());
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    assert!(!executor.has_tasks());
    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn executor_drop_during_thread_local_teardown_is_safe() {
    std::thread::spawn(|| {
        THREAD_LOCAL_EXECUTOR.with(|slot| {
            assert!(slot.replace(Some(Executor::builder().build())).is_none());
        });
        THREAD_LOCAL_EXECUTOR.with(|slot| {
            let mut executor = slot.borrow_mut();
            assert_eq!(
                executor.as_mut().expect("thread-local executor is initialized").execute_cycle(),
                CycleOutcome::Suspend
            );
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

    let mut handle = tasks.add(async { 42 }, TaskOptions::default());

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
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
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.begin_shutdown(ShutdownCause::default());
    let converted = Arc::new(AtomicBool::new(false));

    let mut handle = tasks.add(IntoFutureProbe(Arc::clone(&converted)), TaskOptions::default());

    assert!(!converted.load(Ordering::Acquire));
    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

struct PanickingRemoteJoinWake(AtomicBool);

impl Wake for PanickingRemoteJoinWake {
    #[expect(clippy::panic, reason = "this test injects a wake panic to verify containment")]
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Release);
        panic!("remote join disconnect notification");
    }
}

#[test]
fn disconnected_remote_submission_contains_panicking_join_notification() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let (result_tx, mut remote_join) = RemotePool::new().channel::<usize>();
    let wake = Arc::new(PanickingRemoteJoinWake(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&wake));
    assert!(Pin::new(&mut remote_join).poll(&mut Context::from_waker(&waker)).is_pending());
    drop(executor);

    tasks.add_with_remote_result(async { 42 }, TaskOptions::default(), result_tx);

    assert!(wake.0.load(Ordering::Acquire));
    assert!(matches!(
        Pin::new(&mut remote_join).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(error)) if error.is_shutdown()
    ));
}

#[test]
fn submission_during_shutdown_returns_shutdown_error() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.begin_shutdown(ShutdownCause::default());

    let mut handle = tasks.add(async { 42 }, TaskOptions::default());

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
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

    let _outer = tasks.add(async {}, TaskOptions::observed(sink.transfer_context(), TaskPlacement::Any));

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert!(nested_completed.get());
}

#[test]
fn default_options_emit_lifecycle_events_without_context_enrichment() {
    let (sink, processor) = test_emitter(TEST_ID);
    let mut executor = Executor::builder().sink(sink).build();
    let _handle = executor.tasks().add(async {}, TaskOptions::default());

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

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
        self.tasks.add(
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
    let mut handle = tasks.add(
        ReentrantIntoFuture {
            tasks: tasks.clone(),
            nested_completed: Rc::clone(&nested_completed),
        },
        TaskOptions::default(),
    );

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(42)));
    assert!(nested_completed.get());
}

struct DropOwnerDuringIntoFuture {
    owner: Rc<RefCell<Option<Executor>>>,
    tasks: TaskSet,
    nested: Rc<RefCell<Option<JoinHandle<usize>>>>,
}

impl IntoFuture for DropOwnerDuringIntoFuture {
    type Output = usize;
    type IntoFuture = Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        drop(self.owner.borrow_mut().take());
        let nested = self.tasks.add(async { 42 }, TaskOptions::default());
        assert!(self.nested.replace(Some(nested)).is_none());
        ready(7)
    }
}

#[test]
fn owner_drop_during_conversion_cancels_tasks_and_rejects_nested_submission() {
    let owner = Rc::new(RefCell::new(Some(Executor::builder().build())));
    let tasks = owner.borrow().as_ref().unwrap().tasks();
    let mut existing = tasks.add(pending::<()>(), TaskOptions::default());
    let nested = Rc::new(RefCell::new(None));

    let mut outer = tasks.add(
        DropOwnerDuringIntoFuture {
            owner: Rc::clone(&owner),
            tasks: tasks.clone(),
            nested: Rc::clone(&nested),
        },
        TaskOptions::default(),
    );

    assert!(owner.borrow().is_none());
    assert!(matches!(poll_once(&mut existing), Poll::Ready(Err(error)) if error.is_shutdown()));
    assert!(matches!(poll_once(&mut outer), Poll::Ready(Err(error)) if error.is_shutdown()));
    let mut nested = nested.borrow_mut().take().unwrap();
    assert!(matches!(poll_once(&mut nested), Poll::Ready(Err(error)) if error.is_shutdown()));
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

    let mut outer = tasks.add(pending::<()>(), TaskOptions::default());

    assert!(matches!(poll_once(&mut outer), Poll::Ready(Err(error)) if error.is_shutdown()));
    let mut nested = nested.borrow_mut().take().unwrap();
    assert!(matches!(poll_once(&mut nested), Poll::Ready(Err(error)) if error.is_shutdown()));
}

struct ShutdownDuringIntoFuture {
    tasks: TaskSet,
}

impl IntoFuture for ShutdownDuringIntoFuture {
    type Output = usize;
    type IntoFuture = std::future::Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.tasks.begin_shutdown(ShutdownCause::default());
        std::future::ready(42)
    }
}

#[test]
fn shutdown_during_into_future_conversion_rejects_the_outer_task() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let mut handle = tasks.add(ShutdownDuringIntoFuture { tasks: tasks.clone() }, TaskOptions::default());

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}

struct SubmitOnDrop {
    tasks: TaskSet,
    submitted: Rc<RefCell<Option<JoinHandle<usize>>>>,
}

impl Future for SubmitOnDrop {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for SubmitOnDrop {
    fn drop(&mut self) {
        let handle = self.tasks.add(async { 42 }, TaskOptions::default());
        assert!(self.submitted.replace(Some(handle)).is_none());
    }
}

#[test]
fn cancelled_future_drop_can_submit_and_receive_shutdown_error() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let submitted = Rc::new(RefCell::new(None));
    let _outer_handle = tasks.add(
        SubmitOnDrop {
            tasks: tasks.clone(),
            submitted: Rc::clone(&submitted),
        },
        TaskOptions::default(),
    );
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

    executor.begin_shutdown(ShutdownCause::default());

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    let mut submitted_handle = submitted.borrow_mut().take().expect("future destructor submitted a task");
    assert!(matches!(
        poll_once(&mut submitted_handle),
        Poll::Ready(Err(error)) if error.is_shutdown()
    ));
}

#[test]
fn shutdown_retires_a_batch_of_queued_tasks() {
    const TASK_COUNT: usize = 32;

    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let wakers = Rc::new(RefCell::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..TASK_COUNT {
        handles.push(tasks.add(
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
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    for waker in wakers.borrow().iter() {
        waker.wake_by_ref();
    }

    executor.begin_shutdown(ShutdownCause::default());

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    for mut handle in handles {
        assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
    }
}
