// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public join-handle and executor-lifecycle contracts.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::{Ready, pending, poll_fn, ready};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use arty_executor::remote::Pool as RemotePool;
use arty_executor::{CycleOutcome, Executor, JoinError, JoinHandle, TaskOptions, TaskPlacement, TaskSet};
use observed::Sink;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed_testing::{TEST_ID, test_emitter};

thread_local! {
    static THREAD_LOCAL_EXECUTOR: RefCell<Option<Executor>> = const { RefCell::new(None) };
    static REENTRANT_TELEMETRY_SUBMISSION: RefCell<Option<(TaskSet, Rc<Cell<bool>>)>> = const { RefCell::new(None) };
    static REENTRANT_TELEMETRY_EXECUTOR: RefCell<Option<Rc<Executor>>> = const { RefCell::new(None) };
    static OWNER_DROP_TELEMETRY: RefCell<Option<OwnerDropState>> = const { RefCell::new(None) };
    static CONTEXT_DROP_SUBMISSION: RefCell<Option<ContextDropState>> = const { RefCell::new(None) };
}

struct ReentrantSubmissionProcessor;
struct ReentrantCycleProcessor;
struct OwnerDropProcessor;

struct OwnerDropState {
    owner: Executor,
    tasks: TaskSet,
    nested: Rc<RefCell<Option<JoinHandle<usize>>>>,
}

struct ContextDropState {
    tasks: TaskSet,
    nested: Rc<RefCell<Option<JoinHandle<usize>>>>,
}

struct SubmitOnContextDrop;

impl data_privacy::RedactedDisplay for SubmitOnContextDrop {
    fn fmt(&self, _redactor: &dyn data_privacy::Redactor, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context-drop")
    }
}

impl Drop for SubmitOnContextDrop {
    fn drop(&mut self) {
        let Some(state) = CONTEXT_DROP_SUBMISSION.take() else {
            return;
        };
        let handle = state.tasks.add(async { 42 }, TaskOptions::default());
        assert!(state.nested.replace(Some(handle)).is_none());
    }
}

struct ContextDropEnrichment;

impl observed::enrichment::Enrichment for ContextDropEnrichment {
    fn into_entries(self) -> Vec<observed::__private::EnrichmentEntry> {
        vec![observed::__private::EnrichmentEntry::new("context.drop", SubmitOnContextDrop)]
    }
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

impl EventProcessor for ReentrantCycleProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == "arty.rt.task.spawned"
    }

    fn process(&self, _event: &EventView<'_>) {
        let Some(executor) = REENTRANT_TELEMETRY_EXECUTOR.take() else {
            return;
        };
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
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

#[test]
fn completed_result_survives_executor_drop() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add(async { 42 }, TaskOptions::default());

    assert!(executor.has_tasks());
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert!(!executor.has_tasks());
    drop(executor);

    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(42)));
}

#[test]
fn retained_handle_reports_shutdown_without_delaying_shutdown() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add(pending::<()>(), TaskOptions::default());

    executor.begin_shutdown();

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    assert!(!executor.has_tasks());
    drop(executor);
    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
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
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let mut handle = tasks.add(pending::<()>(), TaskOptions::default());

    tasks.begin_shutdown();
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
            let executor = slot.borrow();
            assert_eq!(
                executor.as_ref().expect("thread-local executor is initialized").execute_cycle(),
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
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.begin_shutdown();
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
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.begin_shutdown();

    let mut handle = tasks.add(async { 42 }, TaskOptions::default());

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn task_created_processor_may_submit_a_nested_task() {
    let sink = Sink::new(
        "arty-executor-reentrant-telemetry",
        vec![Arc::new(ReentrantSubmissionProcessor)],
        tick::SimpleClock::new_frozen(),
    );
    let executor = Executor::builder().sink(sink.clone()).build();
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
    let executor = Executor::builder().sink(sink).build();
    let _handle = executor.tasks().add(async {}, TaskOptions::default());

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

    let events = processor.events();
    assert!(events.iter().any(|event| event.name() == "arty.rt.task.spawned"));
    assert!(events.iter().any(|event| event.name() == "arty.rt.task.succeeded"));
}

#[test]
fn task_created_processor_may_complete_and_release_the_new_task() {
    let sink = Sink::new(
        "arty-executor-reentrant-cycle",
        vec![Arc::new(ReentrantCycleProcessor)],
        tick::SimpleClock::new_frozen(),
    );
    let executor = Rc::new(Executor::builder().sink(sink).build());
    REENTRANT_TELEMETRY_EXECUTOR.set(Some(Rc::clone(&executor)));

    let mut handle = executor.tasks().add(async { 42 }, TaskOptions::default());

    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(42)));
    assert!(!executor.has_tasks());
}

#[test]
fn completed_context_drop_may_submit_a_nested_task() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let nested = Rc::new(RefCell::new(None));
    CONTEXT_DROP_SUBMISSION.set(Some(ContextDropState {
        tasks: tasks.clone(),
        nested: Rc::clone(&nested),
    }));
    let context = Sink::noop().transfer_context().with_enrichment(ContextDropEnrichment);
    let mut outer = tasks.add(async { 7 }, TaskOptions::observed(context, TaskPlacement::Any));

    assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

    assert_eq!(poll_once(&mut outer), Poll::Ready(Ok(7)));
    let mut nested = nested.borrow_mut().take().expect("context destructor submitted a nested task");
    assert_eq!(poll_once(&mut nested), Poll::Ready(Ok(42)));
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
    let executor = Executor::builder().build();
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
    let sink = Sink::new(
        "arty-executor-owner-drop",
        vec![Arc::new(OwnerDropProcessor)],
        tick::SimpleClock::new_frozen(),
    );
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
    executor: Rc<Executor>,
}

impl IntoFuture for ShutdownDuringIntoFuture {
    type Output = usize;
    type IntoFuture = std::future::Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.executor.begin_shutdown();
        std::future::ready(42)
    }
}

#[test]
fn shutdown_during_into_future_conversion_rejects_the_outer_task() {
    let executor = Rc::new(Executor::builder().build());
    let mut handle = executor.tasks().add(
        ShutdownDuringIntoFuture {
            executor: Rc::clone(&executor),
        },
        TaskOptions::default(),
    );

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
    let executor = Executor::builder().build();
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

    executor.begin_shutdown();

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    let mut submitted_handle = submitted.borrow_mut().take().expect("future destructor submitted a task");
    assert!(matches!(
        poll_once(&mut submitted_handle),
        Poll::Ready(Err(error)) if error.is_shutdown()
    ));
}

#[test]
fn shutdown_retires_a_batch_of_queued_tasks() {
    // A representative batch size, not an executor queue-capacity boundary.
    const TASK_COUNT: usize = 32;

    let executor = Executor::builder().build();
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

    executor.begin_shutdown();

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    for mut handle in handles {
        assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
    }
}
