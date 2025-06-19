// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use std::any::type_name;
use std::cell::Cell;
use std::pin::Pin;
use std::rc::Rc;
use std::{task, thread};

use pin_project::{pin_project, pinned_drop};

use crate::{AsyncTask, LocalJoinHandle, once_event};

/// A task that has been accepted by an async worker. The task can either be prepared or unprepard.
/// Prepared tasks can be polled immediately and are created when spawn is executed at a point when
/// everything needed to construct the future is available. Unprepared tasks need to be prepared first
/// and are created when spawn is executed before future construction is possible.
///
/// Ideally, we'd use separate types for prepared and unprepared tasks, but we need to Box<dyn> the task
/// when it's spawned, and we want to avoid an additional allocation for each task.
#[pin_project(PinnedDrop)]
pub struct Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    // This contains either the constructor of the future, or the future (or nothing once the task
    // has been cleared).
    #[pin]
    state: TaskState<F>,

    // Set to None after the result has been set.
    result_tx: Option<once_event::isolated::InefficientSender<R>>,

    // Set to None after the join handle has been acquired for the first time.
    // There can only be one join handle for one task, so this cannot be reused.
    result_rx: Option<once_event::isolated::InefficientReceiver<R>>,

    // In the current implementation this type of task is always inert. However, to assist in
    // verifying code correctness, we still implement the "panic if dropped when not inert" logic
    // and only set this flag when inertness is first queried (after all, we could not be legally
    // dropped before that point because the caller would not know we are inert).
    is_inert: Cell<bool>,

    // Determines whether this task was requested to be aborted.
    is_aborted: Rc<Cell<bool>>,
}

impl<F, R> Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    /// Creates a task directly from a future. This task does not need to be prepared and can be polled immediately.
    ///
    /// # Safety
    ///
    /// When `poll()` has been called at least once, the caller must not drop the instance until
    /// it returns true from `is_inert()`.
    pub unsafe fn new(future: F) -> Pin<Box<Self>> {
        // SAFETY: inside unsafe function retaining safety requirements
        unsafe { Self::new_with_state(TaskState::Prepared(future)) }
    }

    /// # Panics
    ///
    /// Panics if called more than once.
    pub fn join_handle(self: &mut Pin<Box<Self>>) -> LocalJoinHandle<R> {
        let is_aborted = Rc::clone(&self.is_aborted);

        LocalJoinHandle::new(
            self.as_mut()
                .project()
                .result_rx
                .take()
                .expect("join handle for task can only be acquired once"),
            is_aborted,
        )
    }

    /// # Safety
    ///
    /// When `poll()` has been called at least once, the caller must not drop the instance until
    /// it returns true from `is_inert()`.
    unsafe fn new_with_state(state: TaskState<F>) -> Pin<Box<Self>> {
        let (result_tx, result_rx) = once_event::isolated::new_inefficient();
        let is_aborted = Rc::new(Cell::new(false));
        Box::pin(Self {
            state,
            result_tx: Some(result_tx),
            result_rx: Some(result_rx),
            is_inert: Cell::new(false),
            is_aborted,
        })
    }
}

impl<F, R> AsyncTask for Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    // Skip mutation testing because mutating this to "true" will just hang tests.
    #[cfg_attr(test, mutants::skip)]
    fn is_aborted(&self) -> bool {
        self.is_aborted.get()
    }

    // Skip mutation testing because mutating this to "false" will just hang tests.
    #[cfg_attr(test, mutants::skip)]
    fn is_inert(&self) -> bool {
        self.is_inert.set(true);

        // In the current implementation, tasks are always inert. They will stop being so once
        // we get into the performance optimization game.
        true
    }

    fn clear(self: Pin<&mut Self>) {
        self.project().state.clear();
    }
}

impl<F, R> Future for Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    type Output = ();

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();
        let poll_result = {
            let future = this
                .state
                .future()
                .expect("task polled after it was cleared or before it was prepared");

            future.poll(cx)
        };

        match poll_result {
            task::Poll::Ready(result) => {
                // Future API contract allows us to throw if it has already returned a result.
                let result_tx = this.result_tx.take().expect(
                    "we do not consider it legal to poll a task after it has already completed",
                );
                result_tx.set(result);
                task::Poll::Ready(())
            }
            task::Poll::Pending => task::Poll::Pending,
        }
    }
}

#[pinned_drop]
impl<F, R> PinnedDrop for Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    fn drop(self: Pin<&mut Self>) {
        if thread::panicking() {
            // We skip the assertion if we are already panicking because a double panic more often
            // does not help anything and may even obscure the initial panic in test runs.
            return;
        }

        assert!(self.is_inert.get(), "task dropped before it was inert");
    }
}

impl<F, R> fmt::Debug for Task<F, R>
where
    F: Future<Output = R> + 'static,
    R: 'static,
{
    #[cfg_attr(test, mutants::skip)] // We have no contract to test here - can emit anything.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

#[pin_project(project = TaskStateProj, project_replace = TaskStateProjReplace)]
enum TaskState<F> {
    Prepared(#[pin] F),
    Cleared,
}

impl<F> TaskState<F> {
    fn future(self: Pin<&mut Self>) -> Option<Pin<&mut F>> {
        match self.project() {
            TaskStateProj::Prepared(future) => Some(future),
            TaskStateProj::Cleared => None,
        }
    }

    fn clear(self: &mut Pin<&mut Self>) {
        self.set(Self::Cleared);
    }
}

#[cfg(test)]
mod tests {
    use oxidizer_rt_testing::CanaryFuture;

    use super::*;

    #[test]
    fn assert_task_not_send_sync() {
        static_assertions::assert_not_impl_any!(Task<CanaryFuture, ()>: Send, Sync);
    }

    #[test]
    fn clear_drops_future() {
        let (canary, _, observer) = CanaryFuture::new_with_start_notification_and_observer();

        // SAFETY: We have to ensure is_inert() returns true before dropping. We do.
        let mut task = unsafe { Task::new(canary) };

        task.as_mut().clear();

        // There should not be any canary anymore.
        assert!(observer.upgrade().is_none());

        // This test is assuming that the future is inert immediately. This is simply an assumption
        // based on the current simple design of these futures and the test may need to change once
        // we complicate the future design more so it needs extra effort to become inert.
        assert!(task.is_inert());
    }

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    #[expect(clippy::unnecessary_safety_comment, reason = "clippy defect")]
    fn panic_on_dirty_drop() {
        // SAFETY: We intentionally fail to uphold the safety requirements here.
        _ = unsafe { Task::new(async move {}) };
    }

    #[test]
    fn is_aborted_ok() {
        // SAFETY: We have to ensure is_inert() returns true before dropping. We do.
        let mut task = unsafe { Task::new(async { true }) };

        let join_handle = task.join_handle();

        task.as_mut().is_inert();
        join_handle.request_abort();

        assert!(task.is_aborted());
    }
}