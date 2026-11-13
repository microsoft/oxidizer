// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "inspection")]
use std::collections::HashSet;
use std::collections::VecDeque;
use std::num::NonZeroUsize;
#[cfg(feature = "inspection")]
use std::ops::ControlFlow;
#[cfg(feature = "inspection")]
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task;
use std::thread::ThreadId;
use std::time::Instant;

use nm::{Event, Magnitude, MetricsPusher, Push};
use observed::Sink;
use tick::SimpleClock;

use crate::{
    CycleOutcome, ExecutorBuilder, ExecutorShared, Shutdown, ShutdownCause, TaskHandle, TaskList, TaskSet, TypeErasedTask, executor_context,
};
#[cfg(feature = "inspection")]
use crate::{InspectionError, InspectionStatus, PollObserver, TaskPtr, TaskState, discard_task_panic_payload};

/// An async task executor that runs on one thread.
///
/// The executor polls tasks and processes wake notifications so that ready tasks
/// can make progress.
///
/// The executor does not create or manage threads. The caller selects the owner
/// thread and decides what other work runs on that thread. The Arty Runtime uses
/// this executor with I/O and other per-thread services.
///
/// Only one executor can be alive on a thread. Dropping it releases the thread
/// registration, so the caller can create a replacement.
///
/// # Reentrancy and mutability
///
/// Owner operations need exclusive access to the executor. A future can submit
/// tasks through a [`TaskSet`]. The task set does not hold an internal exclusive
/// borrow while it calls external code.
///
/// Some methods do not support reentrant calls. Each method documents its
/// reentrancy requirements.
///
/// # Integration
///
/// The runtime owns and operates the executor. It can give a [`TaskSet`] from
/// [`tasks()`][Self::tasks] to user code. User code can then register tasks,
/// including from a future that runs on the same executor.
///
/// The runtime calls [`execute_cycle()`][Self::execute_cycle] from its main event
/// loop. The returned [`CycleOutcome`] indicates when another cycle is useful.
///
/// The owner can start shutdown at any time. Shutdown cancels active tasks and
/// drops their futures. A later submission through a remaining [`TaskSet`]
/// reports cancellation to its completion target.
///
/// # Shutdown process
///
/// Use this sequence for graceful shutdown:
///
/// 1. Call [`shutdown()`][Self::shutdown] and trigger the returned handle.
/// 2. Keep calling [`execute_cycle()`][Self::execute_cycle] until it returns [`CycleOutcome::Shutdown`].
/// 3. Drop the executor when convenient.
///
/// You can also drop the executor without this sequence. The drop operation
/// cancels remaining tasks before it returns. Retained task wakers keep empty
/// task allocations alive, but the wakers do nothing.
///
/// # Process termination
///
/// The executor contains task panics. [`ExecutorBuilder::panic_policy`] selects
/// whether a task panic starts shutdown.
#[derive(Debug)]
pub struct Executor {
    shared: ExecutorShared,
    active: VecDeque<TaskHandle>,
    pending: TaskList,
    clock: SimpleClock,
    last_cycle_ended: Option<Instant>,
}

/// Selects ordinary or observed polling at monomorphization, not inside the poll loop.
trait ObservePoll: Copy {
    fn poll(self, task: Pin<&dyn TypeErasedTask>, shared: &ExecutorShared) -> task::Poll<()>;
}

impl ObservePoll for () {
    fn poll(self, task: Pin<&dyn TypeErasedTask>, shared: &ExecutorShared) -> task::Poll<()> {
        task.poll(shared)
    }
}

#[cfg(feature = "inspection")]
impl ObservePoll for &dyn PollObserver {
    fn poll(self, task: Pin<&dyn TypeErasedTask>, shared: &ExecutorShared) -> task::Poll<()> {
        let mut observed = false;
        let mut in_callback = false;
        let description = catch_unwind(AssertUnwindSafe(|| {
            task.inspect(TaskState::Running, &mut |description| {
                in_callback = true;
                self.poll_started(description);
                // #[gamma::skip(assign_value.default, reason = "The assigned bool is false, which is exactly bool::default().")]
                in_callback = false;
                observed = true;
            });
        }));
        match description {
            Ok(()) => {}
            Err(panic) if in_callback => resume_unwind(panic),
            Err(panic) => {
                // Observer failures propagate; inspector and description-drop failures
                // cancel the observation before the task itself is polled.
                // #[gamma::skip(assign_value.default, reason = "The assigned bool is false, which is exactly bool::default().")]
                observed = false;
                discard_task_panic_payload(panic);
                self.inspection_failed(InspectionError::Panicked);
            }
        }
        let outcome = task.poll(shared);
        if observed {
            self.poll_finished(outcome.is_ready());
        }
        outcome
    }
}

impl Executor {
    /// Starts building a new instance of [`Executor`].
    pub fn builder() -> ExecutorBuilder {
        ExecutorBuilder::new()
    }

    #[must_use]
    pub(crate) fn new(owner_waker: task::Waker, sink: Sink, thread_id: ThreadId, panic_policy: crate::PanicPolicy) -> Self {
        let shared = ExecutorShared::new(owner_waker, sink, thread_id, panic_policy);
        let clock = SimpleClock::new_system().with_fast_instant(true);
        // Register before creating an executor because its Drop unregisters this thread.
        executor_context::register(thread_id);
        Self {
            shared,
            active: VecDeque::new(),
            pending: TaskList::default(),
            clock,
            last_cycle_ended: None,
        }
    }

    /// Returns a handle that can register tasks with this executor.
    #[must_use]
    pub fn tasks(&self) -> TaskSet {
        TaskSet::new(&self.shared)
    }

    /// Returns a handle that can request executor shutdown.
    #[must_use]
    pub fn shutdown(&self) -> Shutdown {
        self.shared.shutdown().clone()
    }

    /// Returns whether the executor still owns any registered tasks.
    ///
    /// Completed tasks stop counting during the cycle that observes their completion. Retained
    /// wakers do not count as tasks and can remain alive after this returns `false`.
    #[must_use]
    pub fn has_tasks(&self) -> bool {
        self.shared.has_tasks()
    }

    /// Executes one processing cycle to progress registered tasks.
    ///
    /// More work can remain after the cycle. The return value indicates whether
    /// the executor can do more work now.
    ///
    /// Only the owner can call this method with exclusive access. Do not call it
    /// reentrantly.
    ///
    /// # Panics
    ///
    /// Panics if called reentrantly, such as from external code that
    /// `execute_cycle()` calls.
    ///
    /// A panic from a cleanup or notification hook can also propagate. Do not
    /// continue to use the executor after such a panic.
    #[must_use]
    pub fn execute_cycle(&mut self) -> CycleOutcome {
        self.execute_cycle_with(())
    }

    /// Executes one cycle with runtime-owned poll observation.
    ///
    /// Select this entry point only when diagnostics are requested. The ordinary
    /// [`execute_cycle`](Self::execute_cycle) path does not check or call an observer.
    ///
    /// # Panics
    ///
    /// Observer panics propagate. An inspector panic is reported through
    /// [`PollObserver::inspection_failed`](crate::PollObserver::inspection_failed)
    /// and does not prevent the affected task from running.
    #[cfg(feature = "inspection")]
    #[must_use]
    pub fn execute_cycle_observed(&mut self, observer: &dyn PollObserver) -> CycleOutcome {
        self.execute_cycle_with(observer)
    }

    fn execute_cycle_with(&mut self, observer: impl ObservePoll) -> CycleOutcome {
        let _cycle = executor_context::enter_cycle(self.shared.wake().thread_id());
        let cycle_started = self.start_cycle();
        self.accept_new_tasks();
        self.activate_awakened_tasks();
        self.poll_active_tasks(observer);
        self.cancel_if_shutting_down();
        let outcome = self.cycle_outcome();
        self.finish_cycle(cycle_started);
        outcome
    }

    /// Visits at most `limit` native live tasks between execution cycles.
    ///
    /// Tasks without a matching description still count toward the work limit.
    /// Inspection does not consume wakes or move futures. The visitor must not
    /// submit tasks or recursively enter this executor.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero limit, borrowed submission storage, or a panic
    /// from a task inspector or the visitor.
    #[cfg(feature = "inspection")]
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "exclusive owner access prevents inspection from overlapping a poll"
    )]
    pub fn inspect<D: 'static>(&mut self, limit: usize, mut visitor: impl FnMut(&D)) -> Result<InspectionStatus, InspectionError> {
        if limit == 0 {
            return Err(InspectionError::InvalidLimit);
        }
        let _cycle = executor_context::enter_cycle(self.shared.wake().thread_id());
        let inspected = catch_unwind(AssertUnwindSafe(|| {
            let mut count = 0;
            let mut visited = HashSet::new();
            let truncated = std::cell::Cell::new(false);
            let mut inspect = |pointer: TaskPtr, state| {
                // An unwinding observer can leave a runnable task linked in pending.
                if !visited.insert(pointer) {
                    return ControlFlow::Continue(());
                }
                if count == limit {
                    truncated.set(true);
                    return ControlFlow::Break(());
                }
                count += 1;
                // SAFETY: Each queue retains the task throughout this owner-only visit.
                let task = unsafe { pointer.as_task() };
                let state = if task.is_notified() { TaskState::Ready } else { state };
                task.inspect(state, &mut |description| {
                    if let Some(description) = description.downcast_ref::<D>() {
                        visitor(description);
                    }
                });
                ControlFlow::Continue(())
            };
            // #[gamma::skip(try.propagate_to_unwrap, reason = "The owner-only submission queue has no mutable borrow at entry: all queue mutations release their borrows before callbacks, and inspection has not invoked a callback yet.")]
            self.shared.inspect_new_tasks(|handle| inspect(handle.as_ptr(), TaskState::Ready))?;
            // #[gamma::skip(cond.always_true, reason = "After truncation, count equals limit; inspect returns Break before invoking any inspector or visitor, so traversing the next queue cannot change the result.")]
            if !truncated.get() {
                let _traversal = self.active.iter().try_for_each(|handle| inspect(handle.as_ptr(), TaskState::Ready));
            }
            // #[gamma::skip(cond.always_true, reason = "After truncation, count equals limit; inspect returns Break before invoking any inspector or visitor, so traversing the next queue cannot change the result.")]
            if !truncated.get() {
                self.pending.inspect_until(|pointer| inspect(pointer, TaskState::Waiting));
            }
            Ok(if truncated.get() {
                InspectionStatus::Truncated
            } else {
                InspectionStatus::Complete
            })
        }));
        match inspected {
            Ok(result) => result,
            Err(panic) => {
                discard_task_panic_payload(panic);
                Err(InspectionError::Panicked)
            }
        }
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
        if self.shared.shutdown().is_triggered() {
            let cause = self.shared.shutdown().cause();
            let aborted_count = self.prepare_task_abort();
            self.abort_prepared_tasks(aborted_count, true, &cause);
        }
    }

    fn cycle_outcome(&self) -> CycleOutcome {
        if self.evaluate_shutdown_completion() {
            CycleOutcome::Shutdown(self.shared.shutdown().cause())
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

    fn poll_active_tasks(&mut self, observer: impl ObservePoll) {
        let mut completed_count: usize = 0;
        let mut pending_count: usize = 0;

        // Keep the scheduling owner queued until polling and context restoration both return.
        // Otherwise an unwind can leave a remote waker that owns a live non-Send payload.
        while let Some(task_handle) = self.active.front() {
            // SAFETY: The runnable owner keeps the pinned task alive. Only this executor polls
            // and retires its payload on the task's owner thread.
            let task = unsafe { task_handle.as_task() };
            let pointer = task_handle.as_ptr();
            let linked = task.links().linked.get();

            if self.shared.shutdown().is_triggered() {
                task.abort(self.shared.shutdown().cause());
                self.shared.decrement_task_count(1);
                // SAFETY: The active owner keeps `pointer` live. A linked active task belongs to
                // this executor's pending list, and this code runs on the task owner thread.
                drop(unsafe { self.pending.remove(pointer) });
                completed_count = completed_count.saturating_add(1);
                self.active.pop_front();
                continue;
            }

            let outcome = observer.poll(task, &self.shared);
            let task_handle = self.active.pop_front().expect("the task remains active until its poll returns");
            match outcome {
                task::Poll::Ready(()) => {
                    // #[gamma::skip(cond.always_true, reason = "TaskList::remove returns None without modifying the list when the task is not linked.")]
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

        // #[gamma::skip(call_result.default, reason = "The metrics function returns unit and default_after still executes it.")]
        record_poll_metrics(pending_count, completed_count);
    }

    /// Returns whether an immediate cycle can do useful work.
    #[must_use]
    fn has_work_to_do(&self) -> bool {
        debug_assert!(self.active.is_empty(), "active tasks are drained before deciding the next cycle");
        self.shared.has_new_tasks() || executor_context::has_wakes(self.shared.wake().thread_id()) || self.shared.wake().has_remote_wakes()
    }

    fn prepare_task_abort(&mut self) -> usize {
        self.shared.close_remote_wakes();
        executor_context::clear_wakes(self.shared.wake().thread_id());
        for task_handle in &self.active {
            // SAFETY: Each active owner keeps its task live. Any linked task belongs to this
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
                // Metrics TLS can already be destroyed when an executor itself lives in TLS.
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

    #[must_use]
    fn evaluate_shutdown_completion(&self) -> bool {
        if !self.shared.shutdown().is_triggered() {
            return false;
        }
        debug_assert!(self.active.is_empty());
        debug_assert!(self.pending.is_empty());

        self.shared.shutdown_complete()
    }

    fn finish_drop(&mut self, panicking: bool) {
        if panicking {
            // Retire remaining payloads on their owner if cancellation unwinds.
            let cause = self.shared.shutdown().cause();
            let aborted_count = self.prepare_task_abort();
            self.abort_prepared_tasks(aborted_count, false, &cause);
        }
        executor_context::unregister(self.shared.wake().thread_id());
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        struct FinishDrop<'a>(&'a mut Executor);

        impl Drop for FinishDrop<'_> {
            #[cfg_attr(coverage_nightly, coverage(off))] // A second panic during unwinding aborts before a test can observe this safety fallback.
            fn drop(&mut self) {
                self.0.finish_drop(std::thread::panicking());
            }
        }

        _ = self.shared.shutdown().publish(ShutdownCause::default());
        let cause = self.shared.shutdown().cause();
        let guard = FinishDrop(self);
        let aborted_count = guard.0.prepare_task_abort();
        guard.0.abort_prepared_tasks(aborted_count, false, &cause);
        // #[gamma::skip(cond.negate, result.is_err_to_is_ok, reason = "The branch body is empty; either predicate still executes the same try_with call.")]
        if EXECUTOR_PUSHER.try_with(MetricsPusher::push).is_err() {
            // Executor cleanup must remain safe after destruction of metrics TLS.
        }
        debug_assert!(guard.0.shared.shutdown_complete());
    }
}

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

#[cfg(test)]
mod gamma_round2_tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::Poll;
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn nov4_finish_drop_cancels_only_during_unwinding_without_recording_drop_metrics() {
        #[derive(Debug)]
        struct CountCancellation(Rc<std::cell::Cell<usize>>);
        impl crate::TaskCompletion<()> for CountCancellation {
            fn completed(self, (): ()) {
                panic!("pending task cannot complete");
            }
            fn panicked(self, _: crate::TaskPanicPayload, _: Option<ShutdownCause>) {
                panic!("pending task cannot panic");
            }
            fn cancelled(self, _: ShutdownCause) {
                self.0.set(self.0.get() + 1);
            }
        }

        for panicking in [false, true] {
            let cancelled = Rc::new(std::cell::Cell::new(0));
            let mut executor = Executor::builder().build();
            executor
                .tasks()
                .task(std::future::pending())
                .register(CountCancellation(Rc::clone(&cancelled)));
            let dropped_before = metric(&TASKS_DROPPED);
            executor.finish_drop(panicking);
            executor_context::register(executor.shared.wake().thread_id());
            let observed_cancellations = cancelled.get();
            let observed_drop_metric = metric(&TASKS_DROPPED);
            if !executor.active.is_empty() {
                let cause = executor.shared.shutdown().cause();
                executor.abort_prepared_tasks(0, false, &cause);
            }
            drop(executor);
            assert_eq!(observed_cancellations, usize::from(panicking));
            assert_eq!(observed_drop_metric, dropped_before);
            assert_eq!(cancelled.get(), 1);
        }
    }

    #[test]
    fn gamma_round2_new_executor_selects_the_fast_system_instant_source() {
        let executor = Executor::builder().build();
        assert_eq!(format!("{:?}", executor.clock), "SimpleClock(SystemFast)");
    }

    #[test]
    fn gamma_round2_empty_accept_preserves_the_active_queue_allocation() {
        let executor = Executor::builder().build();
        let mut active = VecDeque::with_capacity(23);
        assert_eq!(executor.shared.accept_new_tasks(&mut active), 0);
        assert_eq!(active.capacity(), 23);
    }

    // Inspect owner-thread observations, not process-wide reports that parallel tests also update.
    fn metric(event: &'static std::thread::LocalKey<Event<Push>>) -> (u64, i64) {
        event.with(|event| {
            let debug = format!("{event:?}");
            let count = debug
                .split_once("count: Cell { value: ")
                .unwrap()
                .1
                .split_once('}')
                .unwrap()
                .0
                .trim()
                .parse()
                .unwrap();
            let sum = debug
                .split_once("sum: Cell { value: ")
                .unwrap()
                .1
                .split_once('}')
                .unwrap()
                .0
                .trim()
                .parse()
                .unwrap();
            (count, sum)
        })
    }

    fn assert_delta(event: &'static std::thread::LocalKey<Event<Push>>, before: (u64, i64), count: u64, sum: i64) {
        assert_eq!(metric(event), (before.0 + count, before.1 + sum));
    }

    fn assert_all_observations_pushed() {
        EXECUTOR_PUSHER.with(|pusher| {
            let debug = format!("{pusher:?}");
            let mut pairs = 0;
            for pair in debug.split("LocalGlobalPair {").skip(1) {
                let count = pair
                    .split_once("count: Cell { value: ")
                    .unwrap()
                    .1
                    .split_once('}')
                    .unwrap()
                    .0
                    .trim()
                    .parse::<u64>()
                    .unwrap();
                let pushed = pair
                    .split_once("last_pushed_count: Cell { value: ")
                    .unwrap()
                    .1
                    .split_once('}')
                    .unwrap()
                    .0
                    .trim()
                    .parse::<u64>()
                    .unwrap();
                assert_eq!(pushed, count, "{pair}");
                pairs += 1;
            }
            assert!(pairs > 0);
        });
    }

    #[derive(Debug)]
    struct Completion;

    impl crate::TaskCompletion<()> for Completion {
        fn completed(self, (): ()) {}

        fn panicked(self, _: crate::TaskPanicPayload, _: Option<ShutdownCause>) {
            panic!("the controlled pending task must not panic");
        }

        fn cancelled(self, _: ShutdownCause) {}
    }

    #[test]
    fn gamma_round2_poll_metric_counts_and_magnitudes_are_exact_including_zero() {
        for (pending, completed) in [(0, 0), (1, 0), (0, 1), (3, 2)] {
            let inactive_before = metric(&TASKS_INACTIVATED);
            let completed_before = metric(&TASKS_COMPLETED);
            let dropped_before = metric(&TASKS_DROPPED);
            record_poll_metrics(pending, completed);
            assert_delta(
                &TASKS_INACTIVATED,
                inactive_before,
                u64::from(pending != 0),
                i64::try_from(pending).unwrap(),
            );
            assert_delta(
                &TASKS_COMPLETED,
                completed_before,
                u64::from(completed != 0),
                i64::try_from(completed).unwrap(),
            );
            assert_delta(
                &TASKS_DROPPED,
                dropped_before,
                u64::from(completed != 0),
                i64::try_from(completed).unwrap(),
            );
        }
    }

    #[test]
    fn gamma_round2_accept_poll_reactivate_and_abort_each_report_exact_task_totals() {
        let mut executor = Executor::builder().build();
        executor.clock = SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH);
        let captured = Rc::new(RefCell::new(None));
        let observed = Rc::clone(&captured);
        executor
            .tasks()
            .task(std::future::poll_fn(move |cx| {
                *observed.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }))
            .register(Completion);
        executor.tasks().task(std::future::pending()).register(Completion);
        executor.tasks().task(std::future::ready(())).register(Completion);
        let accepted = metric(&TASKS_ACCEPTED);
        let inactive = metric(&TASKS_INACTIVATED);
        let completed = metric(&TASKS_COMPLETED);
        let dropped = metric(&TASKS_DROPPED);
        let suspended = metric(&CYCLE_OUTCOME_SUSPEND);
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        assert_delta(&TASKS_ACCEPTED, accepted, 1, 3);
        assert_delta(&TASKS_INACTIVATED, inactive, 1, 2);
        assert_delta(&TASKS_COMPLETED, completed, 1, 1);
        assert_delta(&TASKS_DROPPED, dropped, 1, 1);
        assert_delta(&CYCLE_OUTCOME_SUSPEND, suspended, 1, 1);
        let activated = metric(&TASKS_ACTIVATED_VIA_AWAKENED_SET);
        captured.borrow().as_ref().unwrap().wake_by_ref();
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        assert_delta(&TASKS_ACTIVATED_VIA_AWAKENED_SET, activated, 1, 1);
        let continued = metric(&CYCLE_OUTCOME_CONTINUE);
        executor.tasks().task(std::future::pending()).register(Completion);
        assert!(matches!(executor.cycle_outcome(), CycleOutcome::Continue));
        assert_delta(&CYCLE_OUTCOME_CONTINUE, continued, 1, 1);
        let aborted = metric(&TASKS_ABORTED_ON_SHUTDOWN);
        let prepared = executor.prepare_task_abort();
        let dropped = metric(&TASKS_DROPPED);
        executor.abort_prepared_tasks(prepared, true, &ShutdownCause::default());
        assert_eq!(prepared, 3);
        assert_delta(&TASKS_ABORTED_ON_SHUTDOWN, aborted, 3, 3);
        assert_delta(&TASKS_DROPPED, dropped, 1, 3);
        assert!(executor.active.is_empty());
        assert!(executor.pending.is_empty());
    }

    #[test]
    fn gamma_round2_shutdown_during_poll_counts_and_cancels_every_remaining_task() {
        let mut executor = Executor::builder().build();
        executor.clock = SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH);
        let shutdown = executor.shutdown();
        executor
            .tasks()
            .task(async move { shutdown.trigger(ShutdownCause::default()) })
            .register(Completion);
        let polled = Rc::new(std::cell::Cell::new(0));
        for _ in 0..2 {
            let polled = Rc::clone(&polled);
            executor
                .tasks()
                .task(async move { polled.set(polled.get() + 1) })
                .register(Completion);
        }
        executor.accept_new_tasks();
        let completed = metric(&TASKS_COMPLETED);
        let dropped = metric(&TASKS_DROPPED);
        executor.poll_active_tasks(());
        assert_eq!(polled.get(), 0);
        assert!(executor.active.is_empty());
        assert!(executor.pending.is_empty());
        assert_delta(&TASKS_COMPLETED, completed, 1, 3);
        assert_delta(&TASKS_DROPPED, dropped, 1, 3);
    }

    #[test]
    fn gamma_round2_executor_drop_flushes_abort_metrics_without_counting_them_as_normal_drops() {
        let mut executor = Executor::builder().build();
        executor.clock = SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH);
        executor.tasks().task(std::future::pending()).register(Completion);
        executor.tasks().task(std::future::pending()).register(Completion);
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        let aborted = metric(&TASKS_ABORTED_ON_SHUTDOWN);
        let dropped = metric(&TASKS_DROPPED);
        drop(executor);
        assert_delta(&TASKS_ABORTED_ON_SHUTDOWN, aborted, 2, 2);
        assert_delta(&TASKS_DROPPED, dropped, 0, 0);
        assert_all_observations_pushed();
    }

    #[test]
    fn gamma_round2_finish_cycle_records_the_exact_frozen_end_instant() {
        let control = tick::ClockControl::new_at(SystemTime::UNIX_EPOCH);
        let mut executor = Executor::builder().build();
        executor.clock = control.to_simple_clock();
        let duration = metric(&CYCLE_DURATION_MILLIS);
        let gap = metric(&CYCLE_GAP_MILLIS);
        assert!(executor.last_cycle_ended.is_none());
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        assert_eq!(executor.last_cycle_ended, Some(control.to_simple_clock().instant()));
        assert_delta(&CYCLE_DURATION_MILLIS, duration, 1, 0);
        assert_delta(&CYCLE_GAP_MILLIS, gap, 0, 0);
        assert_all_observations_pushed();
        control.advance(Duration::from_millis(7));
        let expected = control.to_simple_clock().instant();
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        assert_eq!(executor.last_cycle_ended, Some(expected));
        assert_delta(&CYCLE_DURATION_MILLIS, duration, 2, 0);
        assert_delta(&CYCLE_GAP_MILLIS, gap, 1, 7);
        assert_all_observations_pushed();
    }

    #[test]
    fn gamma_round2_abort_preparation_clears_queued_wakes_before_task_retirement() {
        let mut executor = Executor::builder().build();
        executor.clock = SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH);
        let captured = Rc::new(RefCell::new(None));
        let observed = Rc::clone(&captured);
        executor
            .tasks()
            .task(std::future::poll_fn(move |cx| {
                *observed.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }))
            .register(Completion);
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        captured.borrow().as_ref().unwrap().wake_by_ref();
        assert!(executor_context::has_wakes(std::thread::current().id()));
        let aborted = executor.prepare_task_abort();
        assert_eq!(aborted, 1);
        assert!(!executor_context::has_wakes(std::thread::current().id()));
        assert_eq!(executor.active.len(), 1);
        assert!(executor.pending.is_empty());
        let owner = std::thread::current().id();
        executor_context::unregister(owner);
        captured.borrow().as_ref().unwrap().wake_by_ref();
        let remote_wake_pending = executor.shared.wake().has_remote_wakes();
        executor_context::register(owner);
        assert!(!remote_wake_pending);
        executor.abort_prepared_tasks(aborted, false, &ShutdownCause::default());
        assert!(executor.active.is_empty());
        captured.borrow().as_ref().unwrap().wake_by_ref();
        assert!(!executor_context::has_wakes(std::thread::current().id()));
    }

    #[test]
    fn gamma_round2_shutdown_cancellation_reports_its_normal_drop_metric() {
        let mut executor = Executor::builder().build();
        executor.tasks().task(std::future::pending()).register(Completion);
        executor.tasks().task(std::future::pending()).register(Completion);
        executor.shutdown().trigger(ShutdownCause::default());
        let dropped = metric(&TASKS_DROPPED);
        executor.cancel_if_shutting_down();
        let still_queued = executor.shared.has_new_tasks();
        drop(executor);
        assert!(!still_queued);
        assert_delta(&TASKS_DROPPED, dropped, 1, 2);
    }

    #[test]
    fn gamma_round2_remote_only_wakes_keep_the_executor_runnable() {
        let mut executor = Executor::builder().build();
        executor.clock = SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH);
        let captured = Rc::new(RefCell::new(None));
        let observed = Rc::clone(&captured);
        executor
            .tasks()
            .task(std::future::poll_fn(move |cx| {
                *observed.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }))
            .register(Completion);
        assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
        let owner = std::thread::current().id();
        executor_context::unregister(owner);
        captured.borrow().as_ref().unwrap().wake_by_ref();
        executor_context::register(owner);
        let remote_only = executor.shared.wake().has_remote_wakes() && !executor_context::has_wakes(owner);
        let runnable = executor.has_work_to_do();
        drop(executor);
        assert!(remote_only);
        assert!(runnable);
    }
}

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
