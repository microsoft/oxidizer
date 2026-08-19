// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::rc::Rc;

use crate::{CycleOutcome, ExecutorBuilder, ExecutorCore, TaskSet};

/// The async task executor for a single thread.
///
/// Its purpose is to continuously poll tasks to ensure the tasks make progress, as well as to
/// provide highly efficient task wake-up services for when tasks are unblocked by the asynchronous
/// operations they are waiting on.
///
/// The executor is single-threaded and does not own or manage threads - it is up to the caller to
/// decide what thread to run it on and what else to do on that thread. This caller is intended to
/// be the Arty Runtime, which combines async task execution, I/O and other fundamental
/// capabilities into a single package of features offered to each thread in an Arty-based
/// application.
///
/// # Reentrancy and mutability
///
/// The executor calls out into external code (futures of the tasks it executes), which may call
/// back into specific methods on the executor (e.g. to enqueue more tasks). Therefore, it operates
/// using internal mutability.
///
/// Not every method can be called in a reentrant manner - see API documentation of individual
/// methods for details.
///
/// # Integration
///
/// The executor is designed to be exclusively owned and operated by the runtime that it services.
/// Instances of [`TaskSet`] obtained from [`tasks()`][Self::tasks] may be shared to user code
/// to facilitate registering additional tasks with the executor. This may be done even from
/// within the futures of existing tasks running on the same executor.
///
/// The runtime that owns the executor is expected to call [`execute_cycle()`][Self::execute_cycle]
/// as part of its main event loop. When exactly the next call should take place is determined by
/// the [`CycleOutcome`] returned from each call.
///
/// The owner may, at any point (regardless of what any registered tasks are doing), initiate the
/// shutdown process and drop the executor after the shutdown process has completed. Any active
/// tasks are aborted and their futures dropped when shutdown starts. Any remaining [`TaskSet`]s
/// become disconnected and will panic if an attempt is made to use them after shutdown starts.
///
/// # Shutdown process
///
/// The executor must be gracefully shut down by the caller before it can be dropped:
///
/// 1. Call [`begin_shutdown()`][Self::begin_shutdown] to start the shutdown process.
/// 2. Keep calling [`execute_cycle()`][Self::execute_cycle] until it returns [`CycleOutcome::Shutdown`].
/// 3. Drop the executor.
///
/// The executor will only return [`CycleOutcome::Shutdown`] when none of its resources are
/// referenced any more (e.g. all join handles have been dropped).
///
/// ## Troubleshooting shutdown failure
///
/// The executor will terminate the process if the shutdown process times out. This is typically
/// a sign of a resource leak that prevented resource manager finalization.
///
/// Potential causes include:
///
/// * Some future awaited by a task failed to cancel an ongoing `await` operation when the future
///   was dropped. This suggests a resource management defect in the future.
/// * A [`JoinHandle`][1] remains alive somewhere with an independent lifetime (e.g. in
///   a `thread_local!` variable). This suggests a resource management defect in whatever logic
///   placed the [`JoinHandle`][1] there.
///
/// If the app is a debug build (`debug_assertions` is set) and `RUST_BACKTRACE=1` is defined,
/// additional diagnostic information will be emitted to standard error stream when the
/// timeout occurs, to help pinpoint what executor resources have not been cleaned up.
///
/// [1]: crate::JoinHandle
#[derive(Debug)]
pub struct Executor {
    core: Rc<ExecutorCore>,
}

impl Executor {
    /// Starts building a new instance of [`Executor`].
    #[cfg_attr(test, mutants::skip)] // Gets mutated into equivalent.
    pub fn builder() -> ExecutorBuilder {
        ExecutorBuilder::new()
    }

    #[must_use]
    pub(crate) fn new(core: ExecutorCore) -> Self {
        Self { core: Rc::new(core) }
    }

    /// Creates a new handle to the set of tasks registered with the executor. You can use
    /// this to register additional tasks.
    #[must_use]
    pub fn tasks(&self) -> TaskSet {
        TaskSet::new(&self.core)
    }

    /// Executes one processing cycle to progress registered tasks.
    ///
    /// More work may remain after a cycle completes. The return value will indicate whether the
    /// executor believes it has more work to do, in which case it should be called again as soon as
    /// possible.
    ///
    /// This function is not safe to call in a reentrant manner - only the owner may
    /// call it in an exclusive context.
    ///
    /// # Panics
    ///
    /// Panics if called in a reentrant manner (e.g. from external code called
    /// from within `execute_cycle()`).
    #[must_use]
    pub fn execute_cycle(&self) -> CycleOutcome {
        self.core.execute_cycle()
    }

    /// Starts the executor shutdown process.
    ///
    /// An executor shutdown may complete instantaneously or occur over many seconds, as it depends
    /// on the release of shared resources that are not under the control of the executor. For
    /// speedy shutdown, ensure that nothing is awaiting on the join handles of tasks registered
    /// with the executor.
    ///
    /// The caller must keep regularly commanding executor cycle execution via
    /// [`execute_cycle()`][1] until they receive [`CycleOutcome::Shutdown`], at which point
    /// the executor is ready to be dropped. Dropping the executor before that point
    /// is a programming error. This is a safety requirement enforced by
    /// [`ExecutorBuilder::build()`][2].
    ///
    /// If the shutdown process takes too long, the executor will declare a timeout and terminate
    /// the process on a call to [`execute_cycle()`][1]. This is a sign of a resource leak.
    ///
    /// If this happens in a debug build and `RUST_BACKTRACE=1` is defined in environment variables,
    /// additional debug information is emitted to the standard error stream to help
    /// you track down the leak.
    ///
    /// This function is not safe to call in a reentrant manner - only the owner may
    /// call it in an exclusive context.
    ///
    /// # Panics
    ///
    /// Panics if called in a reentrant manner (e.g. from external code called
    /// from within [`execute_cycle()`][1]).
    ///
    /// [1]: Self::execute_cycle
    /// [2]: ExecutorBuilder::build
    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub fn begin_shutdown(&self) {
        self.core.begin_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::poll_fn;
    use std::pin::Pin;
    use std::sync::{Arc, atomic};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use static_assertions::assert_not_impl_any;
    use testing_aids::{YieldFuture, assert_panic};

    use super::*;
    use crate::testing::{TestSubjectFuture, TestWaker, new_guarded_executor};
    use crate::{AWAKENED_CAPACITY, ShutdownTimeoutBehavior};

    #[test]
    fn smoke_test() {
        // Create the executor, run a task to completion, and shut the executor down - basic stuff.

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let task_completed = Rc::new(Cell::new(false));

        let future = {
            let task_completed = Rc::clone(&task_completed);

            async move {
                task_completed.set(true);
                42
            }
        };

        let mut join_handle = tasks.add(future);

        // This task completes immediately, so we only need one executor cycle to complete it.
        //
        // In principle, there is no API level guarantee on how many cycles are needed to process
        // a task to completion. This is a white-box test suite, so we hardcode the correct number
        // of cycles. However, it is conceptually fine if changes in implementation strategy change
        // this value - the test simply needs to change to accommodate the new strategy.
        let outcome = executor.execute_cycle();

        // Verify the task signaled it has completed.
        assert!(task_completed.get());

        // The executor must now believe it has no more work to do.
        assert_eq!(outcome, CycleOutcome::Suspend);

        // Verify the join handle provides the result.
        let mut cx = Context::from_waker(Waker::noop());
        let result = Pin::new(&mut join_handle).poll(&mut cx);

        assert!(matches!(result, Poll::Ready(42)));
    }

    #[test]
    fn noop_test() {
        // Creates an executor with no tasks, executes it for a cycle,
        // then observes the universe still exists and goes home.

        let executor = new_guarded_executor(Waker::noop().clone());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn two_tasks_complete_together() {
        // Create the executor, run a task to completion, and shut the executor down - basic stuff.

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future1 = async move { 42 };
        let future2 = async move { 43 };

        let mut join_handle1 = tasks.add(future1);
        let mut join_handle2 = tasks.add(future2);

        // Both tasks complete immediately, so we only need one executor cycle to complete them.
        //
        // In principle, there is no API level guarantee on how many cycles are needed to process
        // a task to completion. This is a white-box test suite, so we hardcode the correct number
        // of cycles. However, it is conceptually fine if changes in implementation strategy change
        // this value - the test simply needs to change to accommodate the new strategy.
        let outcome = executor.execute_cycle();

        // The executor must now believe it has no more work to do.
        assert_eq!(outcome, CycleOutcome::Suspend);

        // Verify the join handle provides the result.
        let mut cx = Context::from_waker(Waker::noop());
        let result1 = Pin::new(&mut join_handle1).poll(&mut cx);
        let result2 = Pin::new(&mut join_handle2).poll(&mut cx);

        assert!(matches!(result1, Poll::Ready(42)));
        assert!(matches!(result2, Poll::Ready(43)));
    }

    #[test]
    fn outcome_continue_on_self_awaken() {
        // A task is polled and immediately awakens itself, causing the executor to request a new
        // execution cycle from the caller immediately after the current cycle completes.

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();
        let wakes_self = future.wakes_self_on_next_poll();

        let _join_handle = tasks.add(future);

        // Enable self-awakening.
        wakes_self.set(true);

        // The first cycle should cause the task to awaken itself and request continue
        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

        // Disable self-awakening for the next poll, so the executor will settle down.
        wakes_self.set(false);

        // The second cycle should inactivate the task (it will never complete in this test).
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn outcome_continue_on_neighbor_awaken() {
        // Two tasks are polled, both suspending immediately and the second one awakens the first
        // one. We expect this to result in a `Continue` outcome.

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future1 = TestSubjectFuture::new();
        let future2 = TestSubjectFuture::new();

        // We do not know which one gets polled first, so they just race to store their waker here.
        let first_waker = Rc::new(Cell::new(None::<Waker>));

        let on_poll = {
            let first_waker = Rc::clone(&first_waker);

            move |cx: &mut Context<'_>| match first_waker.take() {
                // We are the first. Store ours.
                None => first_waker.set(Some(cx.waker().clone())),
                // We are the second. Wake the first.
                Some(waker) => waker.wake(),
            }
        };

        future1.on_poll(on_poll.clone());
        future2.on_poll(on_poll);

        _ = tasks.add(future1);
        _ = tasks.add(future2);

        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn task_completes_after_external_awaken() {
        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();
        let future_waker = future.waker();
        let future_complete = future.completes_on_next_poll();

        let mut join_handle = tasks.add(future);

        // The future should inactivate, leading to a Suspend outcome.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        future_waker.borrow().as_ref().unwrap().wake_by_ref();
        future_complete.set(true);

        // The future should complete, leading to a Suspend outcome.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        let mut cx = Context::from_waker(Waker::noop());
        let result = Pin::new(&mut join_handle).poll(&mut cx);

        // The task must be completed.
        assert!(matches!(result, Poll::Ready(())));
    }

    #[test]
    fn mass_awaken() {
        // We awaken a huge amount of tasks, greater than the capacity of the awakened queue.
        // We still expect them all to awaken together on the same cycle, while one additional
        // inactive task remains suspended when the executor probes all embedded wake signals.
        const AWAKENED_TASK_COUNT: usize = AWAKENED_CAPACITY + 1;

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let mut join_handles = Vec::with_capacity(AWAKENED_TASK_COUNT);
        let mut future_wakers = Vec::with_capacity(AWAKENED_TASK_COUNT);
        let mut future_completes = Vec::with_capacity(AWAKENED_TASK_COUNT);

        for _ in 0..AWAKENED_TASK_COUNT {
            let future = TestSubjectFuture::new();
            future_wakers.push(future.waker());
            future_completes.push(future.completes_on_next_poll());
            join_handles.push(tasks.add(future));
        }

        let inactive_future = TestSubjectFuture::new();
        let mut inactive_join_handle = tasks.add(inactive_future);

        // The futures should all inactivate, leading to a Suspend outcome.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        for future_waker in future_wakers {
            future_waker.borrow().as_ref().unwrap().wake_by_ref();
        }

        for future_complete in future_completes {
            future_complete.set(true);
        }

        // The futures should all complete, leading to a Suspend outcome.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        let mut cx = Context::from_waker(Waker::noop());

        // Every task should now have completed.
        for mut join_handle in join_handles {
            let result = Pin::new(&mut join_handle).poll(&mut cx);
            assert!(matches!(result, Poll::Ready(())));
        }

        assert!(matches!(Pin::new(&mut inactive_join_handle).poll(&mut cx), Poll::Pending));
    }

    #[test]
    fn task_not_polled_when_inactive() {
        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();

        let poll_count = Rc::new(Cell::new(0));

        future.on_poll({
            let poll_count = Rc::clone(&poll_count);
            move |_| {
                poll_count.set(poll_count.get() + 1);
            }
        });

        _ = tasks.add(future);

        // Task gets polled and then inactivates.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        assert_eq!(poll_count.get(), 1);

        // Nothing to poll here, no task has awakened.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        // Note: there is no requirement to only poll a task after it has been awakened. It is fine
        // to poll even without any signal. We rely on white-box knowledge here to establish that
        // no poll is expected.
        assert_eq!(poll_count.get(), 1);
    }

    #[test]
    fn shutdown_drops_futures() {
        // We verify that the future of a task is dropped when we initiate shutdown. This is
        // important to ensure that any resources held by the future's state machine are released.

        struct SetSignalOnDrop(Rc<Cell<bool>>);

        impl Drop for SetSignalOnDrop {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        impl Future for SetSignalOnDrop {
            type Output = ();

            #[cfg_attr(coverage_nightly, coverage(off))] // The test verifies drop before the first poll.
            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Pending
            }
        }

        let drop_signal = Rc::new(Cell::new(false));

        // SAFETY: We promise to not drop this until we get a CycleOutcome of "Shutdown",
        // as required by the Executor.
        let executor = unsafe { Executor::builder().build() };
        let tasks = executor.tasks();

        let signal_setter = SetSignalOnDrop(Rc::clone(&drop_signal));

        tasks.add(signal_setter);

        assert!(!drop_signal.get());

        executor.begin_shutdown();

        assert!(drop_signal.get());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn shutdown_without_tasks_is_immediate() {
        // SAFETY: We promise to not drop this until we get a CycleOutcome of "Shutdown",
        // as required by the Executor.
        let executor = unsafe { Executor::builder().build() };

        executor.begin_shutdown();

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    #[should_panic(expected = "shutdown timeout must be representable")]
    fn unrepresentable_shutdown_timeout_panics_when_shutdown_begins() {
        // SAFETY: The configured timeout intentionally prevents shutdown from beginning.
        let executor = unsafe { Executor::builder().shutdown_timeout(Duration::MAX).build() };

        executor.begin_shutdown();
    }

    #[test]
    fn shutdown_with_unreferenced_tasks_is_immediate() {
        // SAFETY: We promise to not drop this until we get a CycleOutcome of "Shutdown",
        // as required by the Executor.
        let executor = unsafe { Executor::builder().build() };
        let tasks = executor.tasks();

        // We add the task but do not keep any references to its join handle.
        // This task has nothing holding on to its resources, so it can be cleaned up at any time.
        tasks.add(TestSubjectFuture::new());

        executor.begin_shutdown();

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn join_handle_delays_shutdown() {
        // SAFETY: We promise to not drop this until we get a CycleOutcome of "Shutdown",
        // as required by the Executor.
        let executor = unsafe { Executor::builder().build() };
        let tasks = executor.tasks();

        // We add the task and keep a reference to its join handle. This will keep the executor
        // alive until we drop the join handle.
        let join_handle = tasks.add(TestSubjectFuture::new());

        executor.begin_shutdown();

        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        drop(join_handle);

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn join_handle_poll_panics_during_shutdown() {
        // SAFETY: We promise to not drop this until we get a CycleOutcome of "Shutdown",
        // as required by the Executor.
        let executor = unsafe { Executor::builder().build() };
        let tasks = executor.tasks();

        // We add the task and keep a reference to its join handle. This will keep the executor
        // alive until we drop the join handle. The join handle itself will now start to panic,
        // however, as it is impossible for the task to progress to completion.
        let mut join_handle = tasks.add(TestSubjectFuture::new());

        executor.begin_shutdown();

        let mut cx = Context::from_waker(Waker::noop());

        assert_panic!(Pin::new(&mut join_handle).poll(&mut cx));

        // This returns Shutdown because a poll that panics still consumes the join handle and
        // releases resources (it is just a different form of completion, think of this panic as
        // just being a poll().unwrap()).
        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn panic_if_enqueue_after_shutdown() {
        // SAFETY: We promise to not drop this until we get a CycleOutcome of "Shutdown",
        // as required by the Executor.
        let executor = unsafe { Executor::builder().build() };
        let tasks = executor.tasks();

        executor.begin_shutdown();

        let future = async { 42 };
        assert_panic!(tasks.add(future));
    }

    #[test]
    fn panic_if_dirty_drop() {
        // SAFETY: We intentionally fail to uphold the safety promises and skip graceful shutdown.
        let executor = unsafe { Executor::builder().build() };

        assert_panic!(drop(executor));
    }

    #[test]
    fn task_wake_also_wakes_owner() {
        // If something (our test) triggers the waker of a task, this also causes the
        // owner's waker to be triggered.
        let owner_waker = Arc::new(TestWaker::new());

        let executor = new_guarded_executor(Arc::clone(&owner_waker).into());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();
        let future_waker = future.waker();

        tasks.add(future);

        // It just inactivates immediately after storing the waker, so nothing else to do.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        // It should not have been triggered yet, nothing has poked a waker.
        assert!(!owner_waker.awakened.load(atomic::Ordering::Relaxed));

        future_waker.borrow().as_ref().unwrap().wake_by_ref();

        // Now the owner's waker should be triggered as well.
        assert!(owner_waker.awakened.load(atomic::Ordering::Relaxed));
    }

    #[test]
    fn task_awaits_yield_future() {
        // A task awaits a future that yields (Pending on first poll, then immediately completes).

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = async {
            let yield_future = YieldFuture::default();
            yield_future.await;
        };

        _ = tasks.add(future);

        // The task is polled, which suspends due to the inner future's `Pending`.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

        // The inner future triggered the waker, so the task activates again and runs to completion.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn reentrant_task_addition() {
        // We add a new task while already executing another task.

        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let inner_task_completed = Rc::new(Cell::new(false));

        let future = {
            let inner_task_completed = Rc::clone(&inner_task_completed);
            let tasks = tasks.clone();

            async move {
                tasks.add(async move {
                    inner_task_completed.set(true);
                });
            }
        };

        tasks.add(future);

        // The outer task spawns a new task and completes. We get `Continue` because
        // the executor can see it has a new task enqueued, which it wants to process.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

        assert!(!inner_task_completed.get());

        // The inner task now runs to completion.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        assert!(inner_task_completed.get());
    }

    #[test]
    // The difficulty here is that while we could easily assert_panic!() the timeout, this test
    // may also panic for other reasons after the timeout because the timeout is essentially a
    // declaration of failure to clean up - the dirty state is invalid and may result in further
    // panics or even memory safety violations in the test runner. This may break in the future,
    // so be ready to adjust or remove as needed when additional complexity makes it impractical.
    #[should_panic]
    fn shutdown_times_out_with_leaked_join_handle() {
        // SAFETY: We expect to intentionally panic on shutdown due to failed cleanup.
        let executor = unsafe {
            Executor::builder()
                // No delay allowed - shutdown must either succeed or fail immediately.
                .shutdown_timeout(Duration::ZERO)
                // Override the default behavior for testing purposes.
                .shutdown_timeout_behavior(ShutdownTimeoutBehavior::Panic)
                .build()
        };
        let tasks = executor.tasks();

        let leaked_join_handle = tasks.add(async {});

        executor.begin_shutdown();

        // We expect this to panic.
        _ = executor.execute_cycle();

        // We only drop it after shutdown completes, which is a leak because shutdown
        // will never complete like this.
        drop(leaked_join_handle);
    }

    #[test]
    // The difficulty here is that while we could easily assert_panic!() the timeout, this test
    // may also panic for other reasons after the timeout because the timeout is essentially a
    // declaration of failure to clean up - the dirty state is invalid and may result in further
    // panics or even memory safety violations in the test runner. This may break in the future,
    // so be ready to adjust or remove as needed when additional complexity makes it impractical.
    #[should_panic]
    fn shutdown_times_out_with_leaked_waiter() {
        // SAFETY: We expect to intentionally panic on shutdown due to failed cleanup.
        let executor = unsafe {
            Executor::builder()
                // No delay allowed - shutdown must either succeed or fail immediately.
                .shutdown_timeout(Duration::ZERO)
                // Override the default behavior for testing purposes.
                .shutdown_timeout_behavior(ShutdownTimeoutBehavior::Panic)
                .build()
        };
        let tasks = executor.tasks();

        let leaked_waiter = Rc::new(RefCell::new(None));

        tasks.add({
            let leaked_waiter = Rc::clone(&leaked_waiter);

            poll_fn(move |cx| {
                *leaked_waiter.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending::<usize>
            })
        });

        _ = executor.execute_cycle();

        executor.begin_shutdown();

        // We expect this to panic.
        _ = executor.execute_cycle();

        // We only drop it after shutdown completes, which is a leak because shutdown
        // will never complete like this.
        drop(leaked_waiter);
    }

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(Executor: Send, Sync);
    }
}
