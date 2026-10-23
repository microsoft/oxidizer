// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::task;
use std::thread::ThreadId;
use std::time::Instant;

use nm::{Event, Magnitude, MetricsPusher, Push};
use observed::Sink;
use tick::SimpleClock;

use crate::{CycleOutcome, ExecutorBuilder, ExecutorShared, ShutdownCause, TaskHandle, TaskList, TaskSet, executor_context};

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
/// At most one executor may be alive on a thread. Dropping it releases that thread registration,
/// after which a replacement executor may be created.
///
/// # Reentrancy and mutability
///
/// Owner operations take exclusive access to the executor. Futures may still submit tasks through
/// a [`TaskSet`], whose narrowly scoped internal mutability is released around external callbacks.
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
/// shutdown process. Any active tasks are aborted and their futures dropped when shutdown starts.
/// Submissions through remaining [`TaskSet`]s resolve to [`JoinError`](crate::JoinError).
///
/// # Shutdown process
///
/// Graceful shutdown lets the owner observe when cancellation is complete:
///
/// 1. Call [`begin_shutdown()`][Self::begin_shutdown] to start the shutdown process.
/// 2. Keep calling [`execute_cycle()`][Self::execute_cycle] until it returns [`CycleOutcome::Shutdown`].
/// 3. Drop the executor when convenient.
///
/// Dropping the executor without this sequence is also safe and synchronously cancels remaining
/// tasks. Retained join handles and task wakers own independent pooled metadata, so they may
/// outlive the executor and report shutdown or become inert respectively.
///
/// # Process termination
///
/// Plain executor tasks terminate the process if their futures panic. Runtime integrations can
/// register a remote result target that receives captured panics without unwinding the worker.
#[derive(Debug)]
pub struct Executor {
    shared: ExecutorShared,
    active: VecDeque<TaskHandle>,
    pending: TaskList,
    clock: SimpleClock,
    last_cycle_ended: Option<Instant>,
}

impl Executor {
    /// Starts building a new instance of [`Executor`].
    #[cfg_attr(test, mutants::skip)] // Gets mutated into equivalent.
    pub fn builder() -> ExecutorBuilder {
        ExecutorBuilder::new()
    }

    #[must_use]
    pub(crate) fn new(owner_waker: task::Waker, sink: Sink, thread_id: ThreadId) -> Self {
        let shared = ExecutorShared::new(owner_waker, sink, thread_id);
        let clock = SimpleClock::new_system().with_fast_instant(true);
        // Register before creating an executor whose Drop would unregister this thread.
        executor_context::register(thread_id);
        Self {
            shared,
            active: VecDeque::new(),
            pending: TaskList::default(),
            clock,
            last_cycle_ended: None,
        }
    }

    /// Creates a new handle to the set of tasks registered with the executor. You can use
    /// this to register additional tasks.
    #[must_use]
    pub fn tasks(&self) -> TaskSet {
        TaskSet::new(&self.shared)
    }

    /// Returns whether the executor still owns any registered tasks.
    ///
    /// Completed tasks stop counting during the cycle that observes their completion. Join handles
    /// and retained wakers do not count as tasks and may remain alive after this returns `false`.
    #[must_use]
    pub fn has_tasks(&self) -> bool {
        self.shared.has_tasks()
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
    ///
    /// Panics from cleanup and notification hooks may also propagate. Continued operation
    /// after such a panic is not guaranteed.
    #[must_use]
    pub fn execute_cycle(&mut self) -> CycleOutcome {
        let _cycle = executor_context::enter_cycle(self.shared.wake().thread_id());
        let cycle_started = self.start_cycle();
        self.accept_new_tasks();
        self.activate_awakened_tasks();
        self.poll_active_tasks();
        self.cancel_if_shutting_down();
        let outcome = self.cycle_outcome();
        self.finish_cycle(cycle_started);
        outcome
    }

    /// Starts the executor shutdown process.
    ///
    /// The next [`execute_cycle()`][1] returns [`CycleOutcome::Shutdown`] after releasing cancelled
    /// task storage. Retained join handles resolve independently and do not delay shutdown.
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
    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub fn begin_shutdown(&mut self, cause: ShutdownCause) {
        let cause = self.shared.begin_shutdown(cause);
        let aborted_count = self.prepare_task_abort();
        self.abort_prepared_tasks(aborted_count, true, &cause);
    }

    fn start_cycle(&self) -> Instant {
        let started = self.clock.instant();
        if let Some(last_ended) = self.last_cycle_ended {
            CYCLE_GAP_MILLIS.with(|event| {
                event.observe_millis(started.saturating_duration_since(last_ended));
            });
        }
        started
    }

    fn finish_cycle(&mut self, started: Instant) {
        let ended = self.clock.instant();
        self.last_cycle_ended = Some(ended);
        CYCLE_DURATION_MILLIS.with(|event| {
            event.observe_millis(ended.saturating_duration_since(started));
        });
        EXECUTOR_PUSHER.with(MetricsPusher::push);
    }

    fn accept_new_tasks(&mut self) {
        let accepted = self.shared.accept_new_tasks(&mut self.active);
        if let Some(accepted) = NonZeroUsize::new(accepted) {
            TASKS_ACCEPTED.with(|event| event.observe(accepted.get()));
        }
    }

    fn cancel_if_shutting_down(&mut self) {
        if self.shared.is_shutting_down() {
            let cause = self.shared.shutdown_cause();
            let aborted_count = self.prepare_task_abort();
            self.abort_prepared_tasks(aborted_count, true, &cause);
        }
    }

    fn cycle_outcome(&self) -> CycleOutcome {
        if self.evaluate_shutdown_completion() {
            CycleOutcome::Shutdown
        } else if self.has_work_to_do() {
            CYCLE_OUTCOME_CONTINUE.with(Event::observe_once);
            CycleOutcome::Continue
        } else {
            CYCLE_OUTCOME_SUSPEND.with(Event::observe_once);
            CycleOutcome::Suspend
        }
    }

    fn activate_awakened_tasks(&mut self) {
        self.shared.drain_remote_wakes();

        let first_awakened = self.active.len();
        executor_context::append_wakes(self.shared.wake().thread_id(), &mut self.active);
        for task_handle in self.active.range(first_awakened..) {
            // SAFETY: Retirement removes queued notifications before releasing their live tasks.
            unsafe { task_handle.as_task() }.clear_queued_notification();
            TASKS_ACTIVATED_VIA_AWAKENED_SET.with(Event::observe_once);
        }
    }

    fn poll_active_tasks(&mut self) {
        let mut completed_count: usize = 0;
        let mut pending_count: usize = 0;

        // Keep the scheduling owner queued until polling and context restoration both return.
        // Otherwise an unwind could leave a remote waker owning a live non-Send payload.
        while let Some(task_handle) = self.active.front() {
            // SAFETY: The runnable owner keeps the pinned task alive; only this executor polls
            // and retires its payload on the task's owner thread.
            let task = unsafe { task_handle.as_task() };
            let pointer = task_handle.as_ptr();
            let linked = task.links().linked.get();

            if self.shared.is_shutting_down() {
                task.abort(self.shared.shutdown_cause());
                self.shared.decrement_task_count(1);
                // SAFETY: The active owner keeps `pointer` live; a linked active task belongs to
                // this executor's pending list, and this code runs on the task owner thread.
                drop(unsafe { self.pending.remove(pointer) });
                completed_count = completed_count.saturating_add(1);
                self.active.pop_front();
                continue;
            }

            let outcome = task.poll();
            let task_handle = self.active.pop_front().expect("the task remains active until its poll returns");
            match outcome {
                task::Poll::Ready(()) => {
                    if linked {
                        // SAFETY: `linked` was read from the live active task, whose list ownership
                        // belongs to this executor and is accessed on the task owner thread.
                        drop(unsafe { self.pending.remove(pointer) });
                    }
                    self.shared.decrement_task_count(1);
                    completed_count = completed_count.saturating_add(1);
                }
                task::Poll::Pending => {
                    pending_count = pending_count.saturating_add(1);
                    if linked {
                        drop(task_handle);
                    } else {
                        self.pending.insert(task_handle);
                    }
                }
            }
        }

        record_poll_metrics(pending_count, completed_count);
    }

    /// Whether an immediately performed cycle would do useful work.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Mutation can make the executor spin without useful work.
    fn has_work_to_do(&self) -> bool {
        debug_assert!(self.active.is_empty(), "active tasks are drained before deciding the next cycle");
        self.shared.has_new_tasks() || executor_context::has_wakes(self.shared.wake().thread_id()) || self.shared.wake().has_remote_wakes()
    }

    fn prepare_task_abort(&mut self) -> usize {
        self.shared.close_remote_wakes();
        executor_context::clear_wakes(self.shared.wake().thread_id());
        for task_handle in &self.active {
            // SAFETY: Each active owner keeps its task live; any linked task belongs to this
            // executor's pending list, and shutdown preparation runs on the owner thread.
            drop(unsafe { self.pending.remove(task_handle.as_ptr()) });
        }
        self.shared.drain_new_tasks(&mut self.active);
        while let Some(task_handle) = self.pending.pop_front() {
            self.active.push_back(task_handle);
        }
        let aborted_count = self.active.len();
        self.shared.decrement_task_count(aborted_count);
        match EXECUTOR_PUSHER.try_with(|_pusher| TASKS_ABORTED_ON_SHUTDOWN.try_with(|event| event.batch(aborted_count).observe_once())) {
            Ok(Ok(())) => {}
            Ok(Err(_access_error)) | Err(_access_error) => {
                // Metrics TLS may already be destroyed when an executor itself lives in TLS.
            }
        }

        aborted_count
    }

    fn abort_prepared_tasks(&mut self, aborted_count: usize, record_drop_metric: bool, cause: &ShutdownCause) {
        while let Some(task_handle) = self.active.pop_front() {
            // SAFETY: The cancellation batch owns each pinned task on its owner thread.
            unsafe { task_handle.as_task() }.abort(cause.clone());
        }
        if record_drop_metric && let Some(aborted_count) = NonZeroUsize::new(aborted_count) {
            TASKS_DROPPED.with(|event| event.observe(aborted_count.get()));
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    #[must_use]
    fn evaluate_shutdown_completion(&self) -> bool {
        if !self.shared.is_shutting_down() {
            return false;
        }
        debug_assert!(self.active.is_empty());
        debug_assert!(self.pending.is_empty());

        self.shared.shutdown_complete()
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        struct FinishDrop<'a>(&'a mut Executor);

        impl Drop for FinishDrop<'_> {
            #[cfg_attr(coverage_nightly, coverage(off))] // A second panic during unwinding aborts before a test can observe this safety fallback.
            fn drop(&mut self) {
                if std::thread::panicking() {
                    // Retire remaining payloads on their owner if cancellation unwinds.
                    let cause = self.0.shared.shutdown_cause();
                    let aborted_count = self.0.prepare_task_abort();
                    self.0.abort_prepared_tasks(aborted_count, false, &cause);
                }
                executor_context::unregister(self.0.shared.wake().thread_id());
            }
        }

        let cause = self.shared.request_shutdown(ShutdownCause::default());
        let guard = FinishDrop(self);
        let aborted_count = guard.0.prepare_task_abort();
        guard.0.abort_prepared_tasks(aborted_count, false, &cause);
        if EXECUTOR_PUSHER.try_with(MetricsPusher::push).is_err() {
            // Executor cleanup must remain safe after metrics TLS has been destroyed.
        }
        debug_assert!(guard.0.shared.shutdown_complete());
    }
}

#[cfg_attr(test, mutants::skip)] // Just metrics reporting, not practical to test.
fn record_poll_metrics(pending_count: usize, completed_count: usize) {
    if pending_count != 0 {
        TASKS_INACTIVATED.with(|event| event.observe(pending_count));
    }
    if completed_count != 0 {
        TASKS_COMPLETED.with(|event| event.observe(completed_count));
        TASKS_DROPPED.with(|event| event.observe(completed_count));
    }
}

const MILLIS_BUCKETS: &[Magnitude] = &[0, 1, 20, 50, 100, 1_000, 10_000];
const TASKS_TOUCHED_PER_CYCLE_BUCKETS: &[Magnitude] = &[0, 1, 10, 100, 1_000];
const TASKS_MANAGED_PER_CYCLE_BUCKETS: &[Magnitude] = &[0, 1, 10, 100, 1_000];

thread_local! {
    static EXECUTOR_PUSHER: MetricsPusher = MetricsPusher::new();

    static TASKS_ABORTED_ON_SHUTDOWN: Event<Push> = Event::builder()
        .name("ae_tasks_aborted_on_shutdown")
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static TASKS_ACCEPTED: Event<Push> = Event::builder()
        .name("ae_tasks_accepted_per_cycle")
        .histogram(TASKS_MANAGED_PER_CYCLE_BUCKETS)
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static TASKS_COMPLETED: Event<Push> = Event::builder()
        .name("ae_tasks_completed_per_cycle")
        .histogram(TASKS_TOUCHED_PER_CYCLE_BUCKETS)
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static TASKS_INACTIVATED: Event<Push> = Event::builder()
        .name("ae_tasks_inactivated_per_cycle")
        .histogram(TASKS_TOUCHED_PER_CYCLE_BUCKETS)
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static TASKS_DROPPED: Event<Push> = Event::builder()
        .name("ae_tasks_dropped_per_cycle")
        .histogram(TASKS_MANAGED_PER_CYCLE_BUCKETS)
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static TASKS_ACTIVATED_VIA_AWAKENED_SET: Event<Push> = Event::builder()
        .name("ae_tasks_activated_via_awakened_set")
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static CYCLE_DURATION_MILLIS: Event<Push> = Event::builder()
        .name("ae_cycle_duration_millis")
        .histogram(MILLIS_BUCKETS)
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static CYCLE_GAP_MILLIS: Event<Push> = Event::builder()
        .name("ae_cycle_gap_millis")
        .histogram(MILLIS_BUCKETS)
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static CYCLE_OUTCOME_CONTINUE: Event<Push> = Event::builder()
        .name("ae_cycle_outcome_continue")
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static CYCLE_OUTCOME_SUSPEND: Event<Push> = Event::builder()
        .name("ae_cycle_outcome_suspend")
        .pusher_local(&EXECUTOR_PUSHER)
        .build();
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Ready, poll_fn, ready};
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::{Arc, atomic};
    use std::task::{Context, Poll, Waker};

    use static_assertions::assert_not_impl_any;
    use testing_aids::YieldFuture;

    use super::*;
    use crate::executor_context::AWAKENED_CAPACITY;
    use crate::testing::{TestSubjectFuture, TestWaker, new_guarded_executor};
    use crate::{JoinHandle, TaskOptions};

    trait TestTaskSetExt {
        fn add_test<F, R>(&self, future: F) -> JoinHandle<R>
        where
            F: IntoFuture<Output = R> + 'static,
            R: 'static;
    }

    impl TestTaskSetExt for TaskSet {
        fn add_test<F, R>(&self, future: F) -> JoinHandle<R>
        where
            F: IntoFuture<Output = R> + 'static,
            R: 'static,
        {
            self.add(future, TaskOptions::default())
        }
    }

    struct DropOwnerDuringConversion {
        owner: Rc<RefCell<Option<Executor>>>,
        tasks: TaskSet,
        existing_dropped: Rc<Cell<bool>>,
        nested_rejected: Rc<Cell<bool>>,
    }

    struct DropSignalFuture(Rc<Cell<bool>>);

    impl Future for DropSignalFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for DropSignalFuture {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    impl IntoFuture for DropOwnerDuringConversion {
        type Output = usize;
        type IntoFuture = Ready<usize>;

        fn into_future(self) -> Self::IntoFuture {
            drop(self.owner.borrow_mut().take());
            assert!(self.existing_dropped.get(), "owner drop must synchronously cancel existing tasks");
            let mut nested = self.tasks.add_test(async { 42 });
            let mut cx = Context::from_waker(Waker::noop());
            self.nested_rejected.set(matches!(
                Pin::new(&mut nested).poll(&mut cx),
                Poll::Ready(Err(crate::JoinError::Shutdown(_)))
            ));
            ready(7)
        }
    }

    #[test]
    fn owner_drop_during_conversion_cancels_tasks_and_rejects_nested_submission() {
        let owner = Rc::new(RefCell::new(Some(Executor::builder().build())));
        let tasks = owner.borrow().as_ref().unwrap().tasks();
        let existing_dropped = Rc::new(Cell::new(false));
        let mut existing = tasks.add_test(DropSignalFuture(Rc::clone(&existing_dropped)));
        let nested_rejected = Rc::new(Cell::new(false));

        let mut outer = tasks.add_test(DropOwnerDuringConversion {
            owner: Rc::clone(&owner),
            tasks: tasks.clone(),
            existing_dropped: Rc::clone(&existing_dropped),
            nested_rejected: Rc::clone(&nested_rejected),
        });

        assert!(owner.borrow().is_none());
        assert!(existing_dropped.get());
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut existing).poll(&mut cx),
            Poll::Ready(Err(crate::JoinError::Shutdown(_)))
        ));
        assert!(matches!(
            Pin::new(&mut outer).poll(&mut cx),
            Poll::Ready(Err(crate::JoinError::Shutdown(_)))
        ));
        assert!(nested_rejected.get());
    }

    #[test]
    fn smoke_test() {
        // Create the executor, run a task to completion, and shut the executor down - basic stuff.

        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let task_completed = Rc::new(Cell::new(false));

        let future = {
            let task_completed = Rc::clone(&task_completed);

            async move {
                task_completed.set(true);
                42
            }
        };

        let mut join_handle = tasks.add_test(future);

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

        assert!(matches!(result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn noop_test() {
        // Creates an executor with no tasks, executes it for a cycle,
        // then observes the universe still exists and goes home.

        let mut executor = new_guarded_executor(Waker::noop().clone());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn two_tasks_complete_together() {
        // Create the executor, run a task to completion, and shut the executor down - basic stuff.

        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future1 = async move { 42 };
        let future2 = async move { 43 };

        let mut join_handle1 = tasks.add_test(future1);
        let mut join_handle2 = tasks.add_test(future2);

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

        assert!(matches!(result1, Poll::Ready(Ok(42))));
        assert!(matches!(result2, Poll::Ready(Ok(43))));
    }

    #[test]
    fn outcome_continue_on_self_awaken() {
        // A task is polled and immediately awakens itself, causing the executor to request a new
        // execution cycle from the caller immediately after the current cycle completes.

        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();
        let wakes_self = future.wakes_self_on_next_poll();

        let _join_handle = tasks.add_test(future);

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

        let mut executor = new_guarded_executor(Waker::noop().clone());
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

        _ = tasks.add_test(future1);
        _ = tasks.add_test(future2);

        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn task_completes_after_external_awaken() {
        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();
        let future_waker = future.waker();
        let future_complete = future.completes_on_next_poll();

        let mut join_handle = tasks.add_test(future);

        // The future should inactivate, leading to a Suspend outcome.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        future_waker.borrow().as_ref().unwrap().wake_by_ref();
        future_complete.set(true);

        // The future should complete, leading to a Suspend outcome.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        let mut cx = Context::from_waker(Waker::noop());
        let result = Pin::new(&mut join_handle).poll(&mut cx);

        // The task must be completed.
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn mass_awaken() {
        // We awaken a huge amount of tasks, greater than the capacity of the awakened queue.
        // We still expect them all to awaken together on the same cycle, while one additional
        // inactive task remains suspended when the executor probes all embedded wake signals.
        const AWAKENED_TASK_COUNT: usize = AWAKENED_CAPACITY + 1;

        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let mut join_handles = Vec::with_capacity(AWAKENED_TASK_COUNT);
        let mut future_wakers = Vec::with_capacity(AWAKENED_TASK_COUNT);
        let mut future_completes = Vec::with_capacity(AWAKENED_TASK_COUNT);

        for _ in 0..AWAKENED_TASK_COUNT {
            let future = TestSubjectFuture::new();
            future_wakers.push(future.waker());
            future_completes.push(future.completes_on_next_poll());
            join_handles.push(tasks.add_test(future));
        }

        let inactive_future = TestSubjectFuture::new();
        let mut inactive_join_handle = tasks.add_test(inactive_future);

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
            assert!(matches!(result, Poll::Ready(Ok(()))));
        }

        assert!(matches!(Pin::new(&mut inactive_join_handle).poll(&mut cx), Poll::Pending));
    }

    #[test]
    fn task_not_polled_when_inactive() {
        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();

        let poll_count = Rc::new(Cell::new(0));

        future.on_poll({
            let poll_count = Rc::clone(&poll_count);
            move |_| {
                poll_count.set(poll_count.get() + 1);
            }
        });

        _ = tasks.add_test(future);

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

        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();

        let signal_setter = SetSignalOnDrop(Rc::clone(&drop_signal));

        tasks.add_test(signal_setter);

        assert!(!drop_signal.get());

        executor.begin_shutdown(ShutdownCause::default());

        assert!(drop_signal.get());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn shutdown_without_tasks_is_immediate() {
        let mut executor = Executor::builder().build();

        executor.begin_shutdown(ShutdownCause::default());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn task_set_shutdown_is_applied_by_the_next_cycle() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();
        let dropped = Rc::new(Cell::new(false));
        let mut handle = tasks.add_test(DropSignalFuture(Rc::clone(&dropped)));

        tasks.begin_shutdown(ShutdownCause::default());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
        assert!(dropped.get());
        assert!(matches!(
            Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(crate::JoinError::Shutdown(_)))
        ));
    }

    #[test]
    fn shutdown_requested_during_poll_cancels_the_pending_task() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();
        let shutdown = tasks.clone();
        let mut handle = tasks.add_test(poll_fn(move |_| {
            shutdown.begin_shutdown(ShutdownCause::default());
            Poll::Pending::<()>
        }));

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
        assert!(matches!(
            Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(crate::JoinError::Shutdown(_)))
        ));
    }

    #[test]
    fn finishing_a_cycle_records_its_end() {
        let mut executor = Executor::builder().build();
        let started = executor.start_cycle();

        executor.finish_cycle(started);

        assert!(executor.last_cycle_ended.is_some());
    }

    #[test]
    fn shutdown_with_unreferenced_tasks_is_immediate() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();

        // We add the task but do not keep any references to its join handle.
        // This task has nothing holding on to its resources, so it can be cleaned up at any time.
        tasks.add_test(TestSubjectFuture::new());

        executor.begin_shutdown(ShutdownCause::default());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
    }

    #[test]
    fn join_handle_reports_shutdown_without_delaying_shutdown() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();

        let mut join_handle = tasks.add_test(TestSubjectFuture::new());

        executor.begin_shutdown(ShutdownCause::default());

        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
        let mut cx = Context::from_waker(Waker::noop());
        let result = Pin::new(&mut join_handle).poll(&mut cx);

        assert!(matches!(result, Poll::Ready(Err(crate::JoinError::Shutdown(_)))));
    }

    #[test]
    fn enqueue_after_shutdown_reports_shutdown() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();

        executor.begin_shutdown(ShutdownCause::default());

        let mut handle = tasks.add_test(async { 42 });
        let result = Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop()));

        assert!(matches!(result, Poll::Ready(Err(crate::JoinError::Shutdown(_)))));
    }

    #[test]
    fn direct_drop_cancels_tasks_safely() {
        let executor = Executor::builder().build();
        let mut handle = executor.tasks().add_test(std::future::pending::<()>());

        drop(executor);

        let result = Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop()));
        assert!(matches!(result, Poll::Ready(Err(crate::JoinError::Shutdown(_)))));
    }

    #[test]
    fn direct_drop_drains_remote_wakes_while_task_set_is_retained() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();
        let retained = Rc::new(RefCell::new(None));
        tasks.add_test(poll_fn({
            let retained = Rc::clone(&retained);
            move |cx| {
                *retained.borrow_mut() = Some(cx.waker().clone());
                Poll::<()>::Pending
            }
        }));
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        let waker = retained.borrow_mut().take().unwrap();
        std::thread::spawn(move || waker.wake()).join().unwrap();
        assert!(executor.shared.wake().has_remote_wakes());

        drop(executor);

        assert!(tasks.task_storage_is_empty());
    }

    #[test]
    fn direct_drop_unlinks_a_woken_pending_task() {
        let mut executor = Executor::builder().build();
        let future = TestSubjectFuture::new();
        let retained = future.waker();
        let mut handle = executor.tasks().add_test(future);

        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
        retained.borrow().as_ref().unwrap().wake_by_ref();
        executor.activate_awakened_tasks();
        drop(executor);

        let result = Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop()));
        assert!(matches!(result, Poll::Ready(Err(crate::JoinError::Shutdown(_)))));
    }

    #[test]
    fn task_wake_also_wakes_owner() {
        // If something (our test) triggers the waker of a task, this also causes the
        // owner's waker to be triggered.
        let owner_waker = Arc::new(TestWaker::new());

        let mut executor = new_guarded_executor(Arc::clone(&owner_waker).into());
        let tasks = executor.tasks();

        let future = TestSubjectFuture::new();
        let future_waker = future.waker();

        tasks.add_test(future);

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

        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let future = async {
            let yield_future = YieldFuture::default();
            yield_future.await;
        };

        _ = tasks.add_test(future);

        // The task is polled, which suspends due to the inner future's `Pending`.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

        // The inner future triggered the waker, so the task activates again and runs to completion.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    }

    #[test]
    fn reentrant_task_addition() {
        // We add a new task while already executing another task.

        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();

        let inner_task_completed = Rc::new(Cell::new(false));

        let future = {
            let inner_task_completed = Rc::clone(&inner_task_completed);
            let tasks = tasks.clone();

            async move {
                tasks.add_test(async move {
                    inner_task_completed.set(true);
                });
            }
        };

        tasks.add_test(future);

        // The outer task spawns a new task and completes. We get `Continue` because
        // the executor can see it has a new task enqueued, which it wants to process.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

        assert!(!inner_task_completed.get());

        // The inner task now runs to completion.
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        assert!(inner_task_completed.get());
    }

    #[test]
    fn reentrant_tasks_wait_for_next_cycle_in_registration_order() {
        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();
        let polls = Rc::new(RefCell::new(Vec::new()));
        let mut cycles = Vec::new();

        // Recheck registration order after the active and new-task buffers have been swapped and reused.
        for batch in 0..2 {
            for index in 0..2 {
                let tasks_for_child = tasks.clone();
                let polls = Rc::clone(&polls);
                tasks.add_test(async move {
                    polls.borrow_mut().push(batch * 4 + index);
                    tasks_for_child.add_test(async move {
                        polls.borrow_mut().push(batch * 4 + index + 2);
                    });
                });
            }

            cycles.push((executor.execute_cycle(), polls.borrow().clone()));
            cycles.push((executor.execute_cycle(), polls.borrow().clone()));
        }

        assert_eq!(
            cycles,
            [
                (CycleOutcome::Continue, vec![0, 1]),
                (CycleOutcome::Suspend, vec![0, 1, 2, 3]),
                (CycleOutcome::Continue, vec![0, 1, 2, 3, 4, 5]),
                (CycleOutcome::Suspend, vec![0, 1, 2, 3, 4, 5, 6, 7]),
            ]
        );
    }

    #[test]
    fn new_tasks_precede_awakened_tasks() {
        let mut executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();
        let polls = Rc::new(RefCell::new(Vec::new()));

        let future = TestSubjectFuture::new();
        let future_waker = future.waker();
        let completes = future.completes_on_next_poll();
        future.on_poll({
            let polls = Rc::clone(&polls);
            move |_| polls.borrow_mut().push(2)
        });
        tasks.add_test(future);
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
        polls.borrow_mut().clear();

        future_waker.borrow().as_ref().unwrap().wake_by_ref();
        completes.set(true);
        for index in 0..2 {
            let polls = Rc::clone(&polls);
            tasks.add_test(async move {
                polls.borrow_mut().push(index);
            });
        }

        assert_eq!(
            (executor.execute_cycle(), polls.borrow().clone()),
            (CycleOutcome::Suspend, vec![0, 1, 2])
        );
    }

    #[test]
    fn completed_task_waker_is_noop_after_executor_shutdown() {
        let mut executor = Executor::builder().build();
        let waker = Rc::new(RefCell::new(None));
        let polls = Rc::new(Cell::new(0));
        executor.tasks().add_test(poll_fn({
            let waker = Rc::clone(&waker);
            let polls = Rc::clone(&polls);
            move |cx| {
                polls.set(polls.get() + 1);
                *waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Ready(())
            }
        }));

        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
        executor.begin_shutdown(ShutdownCause::default());
        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
        drop(executor);

        waker.borrow().as_ref().unwrap().wake_by_ref();
        assert_eq!(polls.get(), 1);
    }

    #[test]
    fn duplicate_wakes_keep_first_notification_order() {
        let mut executor = new_guarded_executor(Waker::noop().clone());
        let polls = Rc::new(RefCell::new(Vec::new()));
        let mut wakers = Vec::new();
        for index in 0..3 {
            let future = TestSubjectFuture::new();
            wakers.push(future.waker());
            future.on_poll({
                let polls = Rc::clone(&polls);
                move |_| polls.borrow_mut().push(index)
            });
            executor.tasks().add_test(future);
        }
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        // Recheck first-notification order after each cycle clears and reuses the awakened queue.
        for _ in 0..3 {
            polls.borrow_mut().clear();
            for index in [2, 0, 2, 1] {
                wakers[index].borrow().as_ref().unwrap().wake_by_ref();
            }
            assert_eq!(
                (executor.execute_cycle(), polls.borrow().clone()),
                (CycleOutcome::Suspend, vec![2, 0, 1])
            );
        }
    }

    #[test]
    fn cross_thread_waker_clone_wake_and_drop() {
        let mut executor = new_guarded_executor(Waker::noop().clone());
        let future = TestSubjectFuture::new();
        let future_waker = future.waker();
        let completes = future.completes_on_next_poll();
        let mut handle = executor.tasks().add_test(future);
        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

        let waker = future_waker.borrow().as_ref().unwrap().clone();
        std::thread::spawn(move || {
            let clone = waker.clone();
            drop(waker);
            clone.wake();
        })
        .join()
        .unwrap();
        completes.set(true);

        let outcome = executor.execute_cycle();
        let result = Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop()));
        assert_eq!((outcome, result), (CycleOutcome::Suspend, Poll::Ready(Ok(()))));
    }

    #[test]
    fn retained_waiter_does_not_delay_shutdown() {
        let mut executor = Executor::builder().build();
        let tasks = executor.tasks();

        let retained_waiter = Rc::new(RefCell::new(None));

        let task = tasks.add_test({
            let retained_waiter = Rc::clone(&retained_waiter);

            poll_fn(move |cx| {
                *retained_waiter.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending::<usize>
            })
        });

        assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
        let retained_waiter = retained_waiter.borrow_mut().take().unwrap();
        drop(task);
        executor.begin_shutdown(ShutdownCause::default());
        assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
        drop(executor);

        retained_waiter.wake_by_ref();
        retained_waiter.wake();
    }

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(Executor: Send, Sync);
    }

    #[test]
    fn only_one_live_executor_is_allowed_per_thread() {
        let executor = Executor::builder().build();

        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Executor::builder().build()));
        duplicate.unwrap_err();

        drop(executor);
        drop(Executor::builder().build());
    }
}
