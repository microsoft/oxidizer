// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "shared integration-test support is used by different test binaries")]

//! Adapters that turn consuming executor completion callbacks into outcomes
//! that integration tests can poll.

use std::cell::RefCell;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use arty_executor::{ShutdownCause, TaskCompletion, TaskOptions, TaskPanicPayload, TaskSet, discard_task_panic_payload};

pub(crate) enum TaskOutcome<R> {
    Completed(R),
    Panicked(TaskPanicPayload),
    Cancelled(ShutdownCause),
}

/// Shared state that joins one completion target to one awaitable handle.
struct CompletionState<R> {
    outcome: Option<TaskOutcome<R>>,
    waker: Option<Waker>,
}

/// Publishes one executor outcome and contains notification or disposal panics.
pub(crate) struct CompletionTarget<R> {
    state: Rc<RefCell<CompletionState<R>>>,
}

impl<R> fmt::Debug for CompletionTarget<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl<R: 'static> CompletionTarget<R> {
    fn publish(self, outcome: TaskOutcome<R>) {
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| {
            let waker = {
                let mut state = self.state.borrow_mut();
                if state.outcome.is_some() {
                    return;
                }
                state.outcome = Some(outcome);
                state.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        })) {
            discard_task_panic_payload(panic);
        }
    }
}

impl<R: 'static> TaskCompletion<R> for CompletionTarget<R> {
    fn completed(self, result: R) {
        self.publish(TaskOutcome::Completed(result));
    }

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        self.publish(TaskOutcome::Panicked(panic));
    }

    fn cancelled(self, cause: ShutdownCause) {
        self.publish(TaskOutcome::Cancelled(cause));
    }
}

/// Receives the terminal outcome published by its paired completion target.
pub(crate) struct CompletionHandle<R> {
    state: Rc<RefCell<CompletionState<R>>>,
}

impl<R> Future for CompletionHandle<R> {
    type Output = TaskOutcome<R>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        if let Some(outcome) = state.outcome.take() {
            Poll::Ready(outcome)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// Creates paired completion targets and handles for integration tests.
pub(crate) trait TaskSetExt {
    fn add_test<F, R>(&self, future: F, options: TaskOptions) -> CompletionHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static;
}

impl TaskSetExt for TaskSet {
    fn add_test<F, R>(&self, future: F, options: TaskOptions) -> CompletionHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        let state = Rc::new(RefCell::new(CompletionState {
            outcome: None,
            waker: None,
        }));
        self.add(future, options, CompletionTarget { state: Rc::clone(&state) });
        CompletionHandle { state }
    }
}

#[derive(Debug)]
pub(crate) struct DiscardCompletion;

impl<R: 'static> TaskCompletion<R> for DiscardCompletion {
    fn completed(self, result: R) {
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(result))) {
            discard_task_panic_payload(panic);
        }
    }

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}
