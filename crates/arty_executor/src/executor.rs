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

use crate::{CycleOutcome, ExecutorBuilder, ExecutorShared, Shutdown, ShutdownCause, TaskHandle, TaskList, TaskSet, executor_context};

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
/// tasks through a [`TaskSet`]. The task set does not hold an internal mutable
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

impl Executor {
    /// Starts building a new instance of [`Executor`].
    #[cfg_attr(test, mutants::skip)] // Gets mutated into equivalent.
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

    fn poll_active_tasks(&mut self) {
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

            let outcome = task.poll(&self.shared);
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

    /// Returns whether an immediate cycle can do useful work.
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

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    #[must_use]
    fn evaluate_shutdown_completion(&self) -> bool {
        if !self.shared.shutdown().is_triggered() {
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
                    let cause = self.0.shared.shutdown().cause();
                    let aborted_count = self.0.prepare_task_abort();
                    self.0.abort_prepared_tasks(aborted_count, false, &cause);
                }
                executor_context::unregister(self.0.shared.wake().thread_id());
            }
        }

        _ = self.shared.shutdown().publish(ShutdownCause::default());
        let cause = self.shared.shutdown().cause();
        let guard = FinishDrop(self);
        let aborted_count = guard.0.prepare_task_abort();
        guard.0.abort_prepared_tasks(aborted_count, false, &cause);
        if EXECUTOR_PUSHER.try_with(MetricsPusher::push).is_err() {
            // Executor cleanup must remain safe after destruction of metrics TLS.
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
