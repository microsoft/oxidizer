// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public join-handle and executor-lifecycle contracts.

use std::future::pending;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use arty_executor::{CycleOutcome, Executor, JoinError, JoinHandle};

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
