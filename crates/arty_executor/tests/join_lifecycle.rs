// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public join-handle and executor-lifecycle contracts.

use std::cell::{Cell, RefCell};
use std::future::{pending, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use arty_executor::{CycleOutcome, Executor, JoinError, JoinHandle, TaskOptions, TaskPlacement, TaskSet};
use observed::Sink;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};

thread_local! {
    static THREAD_LOCAL_EXECUTOR: RefCell<Option<Executor>> = const { RefCell::new(None) };
    static REENTRANT_TELEMETRY_SUBMISSION: RefCell<Option<(TaskSet, Rc<Cell<bool>>)>> = const { RefCell::new(None) };
}

struct ReentrantSubmissionProcessor;

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
