// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{self, Wake};
use std::{fmt, thread};

use negative_impl::negative_impl;

use crate::{AbstractAsyncTaskExecutor, AsyncTask, WakerFacade};

/// Responsible for progressing asynchronous tasks. The executor is not an active component - it
/// requires someone to explicitly call it to make progress, similar to how futures are polled.
///
/// This is used as an implementation detail of a worker and is not by itself exposed to/via the
/// rest of the runtime.
///
/// # Lifecycle
///
/// The executor must be gracefully shut down by the caller before being dropped, with the shutdown
/// process being:
///
/// 1. Call `shutdown()` to start the shutdown process.
/// 2. Keep executing normally until `CycleResult::Shutdown` is returned from `execute_cycle()`.
///
/// Dropping an instance is only allowed after `CycleResult::Shutdown` is received as a result.
/// If an instance is dropped without going through the shutdown process, the runtime may panic.
#[derive(Debug)]
pub struct AsyncTaskExecutor {
    // The result has not yet been received but the task is ready to be progressed further.
    active: VecDeque<Task>,

    // The result has not yet been received and the task is also not ready to be progressed further.
    inactive: VecDeque<Task>,

    // The task has completed and we are waiting for it to become inert before we drop it.
    completed: VecDeque<Task>,

    shutdown_started: bool,

    thread_waker: WakerFacade,
}

impl AsyncTaskExecutor {
    /// # Safety
    ///
    /// You must receive the `CycleResult::Shutdown` result from `execute_cycle()` before it is
    /// safe to drop the executor (or it will panic).
    pub const unsafe fn new(thread_waker: WakerFacade) -> Self {
        Self {
            active: VecDeque::new(),
            inactive: VecDeque::new(),
            completed: VecDeque::new(),
            shutdown_started: false,
            thread_waker,
        }
    }

    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn activate_awakened_tasks(&mut self) {
        let mut index = 0;

        while index < self.inactive.len() {
            let awakened = self
                .inactive
                .get(index)
                .expect("guarded by while-loop condition")
                .waker
                .awakened
                .swap(false, Ordering::Relaxed);

            if awakened {
                let task = self
                    .inactive
                    .remove(index)
                    .expect("we just used the item at this index, it must exist");
                self.active.push_back(task);
            } else {
                index = index
                    .checked_add(1)
                    .expect("index overflow is inconceivable");
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn poll_active_tasks(&mut self) {
        while let Some(mut task) = self.active.pop_front() {
            // We check on whether the task is aborted before polling it.
            if task.inner.is_aborted() {
                // For aborted tasks, there is nothing that awaits the result, so the aborted task
                // can be immediately completed without polling it again.
                self.complete_task(task);
            } else {
                // We assert unwind safety not because we believe the task to be unwind safe (and
                // certainly any shared resources would not be) but because we expect the task to
                // not be executed again after a panic, so the risk of damage is constrained.
                // Shared resources may still remain in an invalid state but that is the price of
                // using panics for flow control. Do not do that. This is simply an attempt to avoid
                // misleading double panics and to help ensure that the shutdown process is orderly.
                let result = match catch_unwind(AssertUnwindSafe(|| task.poll())) {
                    Ok(result) => result,
                    Err(p) => {
                        // If the task panics, we shove it back into the active set and rethrow
                        // the panic. We expect the caller to (potentially) start the shutdown
                        // process as a result of the panic. The only real important thing here is
                        // that we cannot just drop the task as it needs to go through its proper
                        // shutdown process to avoid an unnecessary multiple layers of panic.
                        self.active.push_back(task);

                        resume_unwind(p);
                    }
                };

                match result {
                    task::Poll::Ready(()) => {
                        self.complete_task(task);
                    }
                    task::Poll::Pending => {
                        self.inactive.push_back(task);
                    }
                }
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Critical because runtime will hang on shutdown if mutated.
    fn drop_inert_tasks(&mut self) {
        self.completed.retain(|task| !task.inner.is_inert());
    }

    /// Returns whether there is any work to do for the executor. This is used to determine if the
    /// executor should be called again immediately or if execution should be suspended until new
    /// work arrives.
    fn has_work_to_do(&self) -> bool {
        !self.active.is_empty()
            || self
                .inactive
                .iter()
                .any(|x| x.waker.awakened.load(Ordering::Relaxed))
    }

    fn complete_task(&mut self, mut task: Task) {
        task.inner.as_mut().clear();
        self.completed.push_back(task);
    }
}

impl AbstractAsyncTaskExecutor for AsyncTaskExecutor {
    /// Enqueues a new task to be processed by the executor.
    ///
    /// # Panics
    ///
    /// Panics if the executor is already shutting down. It is the responsibility of the worker
    /// that owns the executor to ensure that no new tasks are enqueued after the executor has
    /// started the shutdown process.
    fn enqueue(&mut self, task: Pin<Box<dyn AsyncTask>>) {
        assert!(
            !self.shutdown_started,
            "cannot enqueue new tasks after executor shutdown has started"
        );

        // SAFETY: We are required to not drop the task until task.inner.is_inert(). We fulfill this
        // behavior via our own "must not drop executor until shutdown completes" requirement.
        let task = unsafe { Task::new(task, self.thread_waker.clone()) };
        self.active.push_back(task);
    }

    /// Executes one processing cycle to progress registered tasks.
    ///
    /// More work may remain after a cycle completes. The return value will indicate whether the
    /// executor believes it has more work to do, in which case it should be called again as soon as
    /// possible.
    fn execute_cycle(&mut self) -> CycleResult {
        self.activate_awakened_tasks();
        self.poll_active_tasks();
        self.drop_inert_tasks();

        if self.shutdown_started && self.completed.is_empty() {
            // We cannot possibly have non-completed tasks when we are shutting down - the whole
            // point of the shutdown process is that we consider all tasks immediately completed
            // and begin winding them down to a state wherein they can be dropped.
            assert!(
                self.active.is_empty(),
                "active tasks must be empty during shutdown"
            );
            assert!(
                self.inactive.is_empty(),
                "inactive tasks must be empty during shutdown"
            );

            // The shutdown process has finished when all tasks have become inert and
            // have been dropped.
            CycleResult::Shutdown
        } else if self.has_work_to_do() {
            // We want to be immediately called again because we have more work to do.
            CycleResult::Continue
        } else {
            // We have no work to do, feel free to take a while before coming back to us.
            CycleResult::Suspend
        }
    }

    /// Starts the safe shutdown process. The caller must keep normally using the executor until
    /// it receives `CycleResult::Shutdown` from `execute_cycle()`. Only after that may the executor
    /// be dropped.
    #[cfg_attr(test, mutants::skip)] // Critical because runtime will hang on drop if mutated.
    fn begin_shutdown(&mut self) {
        self.shutdown_started = true;

        // All tasks are now considered completed, regardless of their previous state.
        let tasks = self
            .active
            .drain(..)
            .chain(self.inactive.drain(..))
            .collect::<Vec<_>>();

        for task in tasks {
            self.complete_task(task);
        }
    }
}

impl Drop for AsyncTaskExecutor {
    fn drop(&mut self) {
        if thread::panicking() {
            // We skip the assertions if we are already panicking because a double panic more often
            // does not help anything and may even obscure the initial panic in test runs.
            return;
        }

        assert!(
            self.shutdown_started,
            "{} dropped without safe shutdown process",
            type_name::<Self>(),
        );

        assert!(
            self.active.is_empty(),
            "active tasks must be empty during drop"
        );
        assert!(
            self.inactive.is_empty(),
            "inactive tasks must be empty during drop"
        );
        assert!(
            self.completed.is_empty(),
            "completed tasks must be empty during drop"
        );
    }
}

#[negative_impl]
impl !Send for AsyncTaskExecutor {}
#[negative_impl]
impl !Sync for AsyncTaskExecutor {}

/// The result of executing one processing cycle of the async task executor.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum CycleResult {
    /// The processing cycle was completed and the executor is requesting a new processing cycle as
    /// soon as possible because it believes it already has more work to do.
    Continue,

    /// The processing cycle was completed and the executor is confident that no further progress
    /// can be made at this time.
    ///
    /// The caller should avoid executing further processing cycles until there is reason for the
    /// caller to suspect additional progress can be made. Examples of such reasons include:
    /// * The caller has enqueued new tasks.
    /// * An I/O operation has completed.
    /// * A cross-thread wake notification has arrived.
    Suspend,

    /// The executor has completed shutdown and is ready to be dropped.
    Shutdown,
}

/// The executor's internal view of a task, combining an `AsyncTask` with execution-related state.
struct Task {
    inner: Pin<Box<dyn AsyncTask>>,

    waker: Arc<VeryInefficientWaker>,
}

impl Task {
    /// # Safety
    ///
    /// It is invalid to drop an instance before `inner.is_inert()` returns true.
    unsafe fn new(inner: Pin<Box<dyn AsyncTask>>, thread_waker: WakerFacade) -> Self {
        Self {
            inner,
            waker: Arc::new(VeryInefficientWaker {
                awakened: AtomicBool::new(false),
                thread_waker,
            }),
        }
    }

    fn poll(&mut self) -> task::Poll<()> {
        let wrapped_waker = Arc::clone(&self.waker).into();
        let mut context = task::Context::from_waker(&wrapped_waker);

        self.inner.as_mut().poll(&mut context)
    }
}

impl fmt::Debug for Task {
    #[cfg_attr(test, mutants::skip)] // We have no contract to test here - can return anything.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task").finish()
    }
}

/// Very inefficient placeholder implementation that works but is wasteful of system resources.
/// To be replaced in 2025 once we start to invest into performance work in the runtime.
#[derive(Debug)]
struct VeryInefficientWaker {
    awakened: AtomicBool,
    thread_waker: WakerFacade,
}

impl Wake for VeryInefficientWaker {
    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn wake(self: Arc<Self>) {
        self.awakened.store(true, Ordering::Relaxed);
        self.thread_waker.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::Waker;
    use std::time::Instant;

    use mockall::Sequence;
    use oxidizer_testing::TEST_TIMEOUT;
    use scopeguard::{Always, ScopeGuard};

    use super::*;
    use crate::{MockAsyncTask, ThreadWaker};

    #[test]
    fn smoke_test() {
        // Create the executor, run a task to completion, and shut the executor down - basic stuff.

        let mut executor = new_guarded_executor();

        let mut task = MockAsyncTask::new();

        let mut seq = Sequence::new();

        // First, the task is checked on whether it is aborted.
        task.expect_is_aborted()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(false);

        // The task is polled once, returns ready, and is never polled again.
        task.expect_poll()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(task::Poll::Ready(()));

        // All completed tasks must be cleared to move them into the Dead state.
        task.expect_clear()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());

        // Once in the Dead state, we expect to be asked whether we are inert so we can be dropped.
        task.expect_is_inert()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(true);

        executor.enqueue(Box::pin(task));

        // In principle, there is no API level guarantee on how many cycles are needed to process
        // a task to completion. This is a white-box test suite, so we hardcode the correct number
        // of cycles. However, it is perfectly fine if changes in implementation strategy change
        // this value - the test simply needs to change to accommodate the new strategy.

        // This one cycle should be enough to do everything with the task - poll, clear, drop.
        // There is no work left to do, so the executor should ask the caller to suspend.
        assert_eq!(executor.execute_cycle(), CycleResult::Suspend);
    }

    #[test]
    fn requests_continue_if_has_more_work() {
        // A task is polled and immediately awakens itself, causing the executor to request a new
        // execution cycle from the caller immediately after the current cycle completes.

        let mut executor = new_guarded_executor();

        let mut task = MockAsyncTask::new();

        let mut seq = Sequence::new();

        // First, the task is checked on whether it is aborted.
        task.expect_is_aborted()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(false);

        // The task is polled once, awakens itself immediately, and returns Pending.
        task.expect_poll()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|cx| {
                cx.waker().wake_by_ref();
                task::Poll::Pending
            });

        // All completed tasks must be cleared to move them into the Dead state.
        task.expect_clear()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());

        // Once in the Dead state, we expect to be asked whether we are inert so we can be dropped.
        task.expect_is_inert()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(true);

        executor.enqueue(Box::pin(task));

        // In principle, there is no API level guarantee on how many cycles are needed to process
        // a task to completion. This is a white-box test suite, so we hardcode the correct number
        // of cycles. However, it is perfectly fine if changes in implementation strategy change
        // this value - the test simply needs to change to accommodate the new strategy.

        // The self-awaken should create more work, causing it to request one more cycle.
        assert_eq!(executor.execute_cycle(), CycleResult::Continue);
    }

    #[test]
    fn wake() {
        // First poll of the task returns Pending, then we cause the task to wake up,
        // after which the next cycle completes the task.

        let mut executor = new_guarded_executor();

        let mut task = MockAsyncTask::new();

        let mut seq = Sequence::new();

        // Value set when first poll is performed.
        let waker: Rc<RefCell<Option<Waker>>> = Rc::new(RefCell::new(None));

        // First, the task is checked on whether it is aborted.
        task.expect_is_aborted().times(2).return_const(false);

        // First poll, it returns Pending, second poll returns ready.
        task.expect_poll()
            .times(1)
            .in_sequence(&mut seq)
            .returning_st({
                let waker = Rc::clone(&waker);

                move |cx| {
                    *waker.borrow_mut() = Some(cx.waker().clone());
                    task::Poll::Pending
                }
            });

        task.expect_poll()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(task::Poll::Ready(()));

        // All completed tasks must be cleared to move them into the Dead state.
        task.expect_clear()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());

        // Once in the Dead state, we expect to be asked whether we are inert so we can be dropped.
        task.expect_is_inert()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(true);

        executor.enqueue(Box::pin(task));

        // In principle, there is no API level guarantee on how many cycles are needed to process
        // a task to completion. This is a white-box test suite, so we hardcode the correct number
        // of cycles. However, it is perfectly fine if changes in implementation strategy change
        // this value - the test simply needs to change to accommodate the new strategy.

        // The first cycle should do the first poll and then consider the task inactive.
        // There is no work left to do, so the executor should ask the caller to suspend.
        assert_eq!(executor.execute_cycle(), CycleResult::Suspend);

        waker
            .borrow_mut()
            .take()
            .expect("waker not obtained - first poll never occurred")
            .wake();

        // The second cycle should do the second poll, complete the task and drop it.
        // There is no work left to do, so the executor should ask the caller to suspend.
        assert_eq!(executor.execute_cycle(), CycleResult::Suspend);
    }

    #[test]
    fn delayed_inertitude() {
        // The task completes right away but takes an extra cycle to become inert.

        let mut executor = new_guarded_executor();

        let mut task = MockAsyncTask::new();

        let mut seq = Sequence::new();

        // First, the task is checked on whether it is aborted.
        task.expect_is_aborted()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(false);

        task.expect_poll()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(task::Poll::Ready(()));

        // All completed tasks must be cleared to move them into the Dead state.
        task.expect_clear()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());

        // Once in the Dead state, we expect to be asked whether we are inert so we can be dropped.
        // The first time we are asked we say no. The second time we say yes.
        // First poll, it returns Pending, second poll returns ready.
        task.expect_is_inert()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(false);

        task.expect_is_inert()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(true);

        executor.enqueue(Box::pin(task));

        // In principle, there is no API level guarantee on how many cycles are needed to process
        // a task to completion. This is a white-box test suite, so we hardcode the correct number
        // of cycles. However, it is perfectly fine if changes in implementation strategy change
        // this value - the test simply needs to change to accommodate the new strategy.

        // The first cycle should do the poll and clear the task, moving it to the completed state.
        // There is no work left to do, so the executor should ask the caller to suspend.
        assert_eq!(executor.execute_cycle(), CycleResult::Suspend);

        // The second cycle should do the second inertness check and drop the task.
        // There is no work left to do, so the executor should ask the caller to suspend.
        assert_eq!(executor.execute_cycle(), CycleResult::Suspend);
    }

    #[test]
    fn aborted_task_ensure_cleared() {
        let mut executor = new_guarded_executor();
        let mut task = MockAsyncTask::new();
        let mut seq = Sequence::new();

        // First, the task is checked on whether it is aborted.
        task.expect_is_aborted()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(true);

        // Aborted tasks are immediately cleared.
        task.expect_clear()
            .times(1)
            .in_sequence(&mut seq)
            .return_const(());

        task.expect_is_inert().times(1).return_const(true);
        task.expect_poll().times(0);

        executor.enqueue(Box::pin(task));

        assert_eq!(executor.execute_cycle(), CycleResult::Suspend);
    }

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn panic_if_enqueue_after_shutdown() {
        let mut executor = new_guarded_executor();
        executor.begin_shutdown();

        let task = MockAsyncTask::new();
        executor.enqueue(Box::pin(task));
    }

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn panic_if_dirty_drop() {
        // SAFETY: We intentionally fail to uphold the safety promises and skip graceful shutdown.
        let _executor = unsafe { AsyncTaskExecutor::new(ThreadWaker::new().into()) };
    }

    fn new_guarded_executor() -> ScopeGuard<AsyncTaskExecutor, fn(AsyncTaskExecutor), Always> {
        scopeguard::guard(
            // SAFETY: We are not allowed to drop it without the proper shutdown process.
            // That is the whole point of this guard, so we are all good on that front.
            unsafe { AsyncTaskExecutor::new(ThreadWaker::new().into()) },
            |mut executor: AsyncTaskExecutor| {
                executor.begin_shutdown();

                let shutdown_started = Instant::now();

                while executor.execute_cycle() != CycleResult::Shutdown {
                    // There is nothing else for us to do but to keep going.
                    thread::yield_now();

                    assert!(
                        shutdown_started.elapsed() < TEST_TIMEOUT,
                        "shutdown timeout - executor probably deadlocked"
                    );
                }
            },
        )
    }
}