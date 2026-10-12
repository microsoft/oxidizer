// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public join-handle and executor-lifecycle contracts.

use std::cell::{Cell, RefCell};
use std::future::{pending, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use arty_executor::{CycleOutcome, Executor, JoinError, JoinHandle, TaskSet};

fn poll_once<R: 'static>(handle: &mut JoinHandle<R>) -> Poll<Result<R, JoinError>> {
    Pin::new(handle).poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn completed_result_survives_executor_drop() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add(async { 42 });

    assert!(executor.has_tasks());
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    assert!(!executor.has_tasks());
    drop(executor);

    assert_eq!(poll_once(&mut handle), Poll::Ready(Ok(42)));
}

#[test]
fn retained_handle_reports_shutdown_without_delaying_shutdown() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add(pending::<()>());

    executor.begin_shutdown();

    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    assert!(!executor.has_tasks());
    drop(executor);
    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn direct_executor_drop_cancels_tasks_safely() {
    let executor = Executor::builder().build();
    let mut handle = executor.tasks().add(pending::<()>());

    drop(executor);

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn disconnected_task_set_returns_shutdown_error() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    drop(executor);

    let mut handle = tasks.add(async { 42 });

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

#[test]
fn submission_during_shutdown_returns_shutdown_error() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    executor.begin_shutdown();

    let mut handle = tasks.add(async { 42 });

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
}

struct ReentrantIntoFuture {
    tasks: TaskSet,
    nested_completed: Rc<Cell<bool>>,
}

impl IntoFuture for ReentrantIntoFuture {
    type Output = usize;
    type IntoFuture = std::future::Ready<usize>;

    fn into_future(self) -> Self::IntoFuture {
        self.tasks.add({
            let nested_completed = Rc::clone(&self.nested_completed);
            async move {
                nested_completed.set(true);
            }
        });
        std::future::ready(42)
    }
}

#[test]
fn into_future_conversion_may_submit_a_nested_task() {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let nested_completed = Rc::new(Cell::new(false));
    let mut handle = tasks.add(ReentrantIntoFuture {
        tasks: tasks.clone(),
        nested_completed: Rc::clone(&nested_completed),
    });

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
    let mut handle = executor.tasks().add(ShutdownDuringIntoFuture {
        executor: Rc::clone(&executor),
    });

    assert!(matches!(poll_once(&mut handle), Poll::Ready(Err(error)) if error.is_shutdown()));
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}

#[test]
fn shutdown_retires_a_batch_of_queued_tasks() {
    const TASK_COUNT: usize = 32;

    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    let wakers = Rc::new(RefCell::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..TASK_COUNT {
        handles.push(tasks.add({
            let wakers = Rc::clone(&wakers);
            poll_fn(move |cx| {
                wakers.borrow_mut().push(cx.waker().clone());
                Poll::<()>::Pending
            })
        }));
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
