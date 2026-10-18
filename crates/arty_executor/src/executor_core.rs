// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::sync::Arc;
use std::task;
use std::time::Instant;

use nm::{Event, Magnitude, MetricsPusher, Push};
use observed::Sink;
use tick::SimpleClock;

use crate::wake::WakeShared;
use crate::{CycleOutcome, ExecutorShared, TaskList, TaskRef};

/// Owner-only executor state.
#[derive(Debug)]
pub(crate) struct ExecutorCore {
    shared: ExecutorShared,
    active: VecDeque<TaskRef>,
    tasks: TaskList,
    clock: SimpleClock,
    last_cycle_ended: Option<Instant>,
}

// Owner-thread wake notifications use the executor's TLS queue without synchronization. Remote
// cloned wakers first travel through the executor's unbounded performables channel and are
// converted to owner-thread notifications while draining that channel.
//
// It is just a VecDeque so the size does not significantly impact wake-up processing time.
// This is the initial TLS queue capacity; VecDeque may grow if a cycle receives more notifications.
#[cfg(not(miri))]
pub(crate) const AWAKENED_CAPACITY: usize = 1024;

// Miri performance scales with memory access count, so large numbers are our enemy.
#[cfg(miri)]
pub(crate) const AWAKENED_CAPACITY: usize = 4;

impl ExecutorCore {
    /// Creates a new async task executor.
    ///
    /// The `owner_waker` is used by the executor to wake up its owner when more work has
    /// arrived for the executor and calling `execute_cycle()` is desirable.
    #[must_use]
    pub(crate) fn new(owner_waker: task::Waker, sink: Sink) -> Self {
        Self {
            shared: ExecutorShared::new(owner_waker, sink),
            active: VecDeque::new(),
            tasks: TaskList::default(),
            clock: SimpleClock::new_system().with_fast_instant(true),
            last_cycle_ended: None,
        }
    }

    pub(crate) fn register_current(&self) {
        self.shared.register_current();
    }

    pub(crate) fn shared(&self) -> &ExecutorShared {
        &self.shared
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.shared.has_tasks()
    }

    #[must_use]
    pub(crate) fn execute_cycle(&mut self) -> CycleOutcome {
        let shared = &self.shared;
        let _cycle = shared.wake().enter_cycle();

        let cycle_start_timestamp = self.clock.instant();

        if let Some(last_cycle_ended) = self.last_cycle_ended {
            let cycle_gap = cycle_start_timestamp.saturating_duration_since(last_cycle_ended);
            CYCLE_GAP_MILLIS.with(|x| x.observe_millis(cycle_gap));
        }

        let accepted = shared.accept_new_tasks(&mut self.active);
        if accepted != 0 {
            TASKS_ACCEPTED.with(|x| x.observe(accepted));
        }
        Self::activate_awakened_tasks(shared.wake(), &mut self.active);

        Self::poll_active_tasks(shared, &mut self.active, &mut self.tasks);

        if shared.is_shutting_down() {
            let aborted_count = Self::prepare_task_abort(shared, &mut self.active, &mut self.tasks);
            Self::abort_prepared_tasks(&mut self.active, aborted_count, true);
        }

        let outcome = if Self::evaluate_shutdown_completion(shared, &self.active, &self.tasks) {
            CycleOutcome::Shutdown
        } else if Self::has_work_to_do(shared, &self.active) {
            // We want to be immediately called again because we may have more work to do.
            CYCLE_OUTCOME_CONTINUE.with(Event::observe_once);
            CycleOutcome::Continue
        } else {
            // We have no work to do, feel free to take a while before coming back to us.
            // We will try to trigger the owner's waker if more work arrives, though this is
            // not guaranteed.
            CYCLE_OUTCOME_SUSPEND.with(Event::observe_once);
            CycleOutcome::Suspend
        };

        let cycle_end_timestamp = self.clock.instant();

        let cycle_duration = cycle_end_timestamp.saturating_duration_since(cycle_start_timestamp);
        self.last_cycle_ended = Some(cycle_end_timestamp);
        CYCLE_DURATION_MILLIS.with(|x| x.observe_millis(cycle_duration));

        // Publish metrics from this cycle.
        EXECUTOR_PUSHER.with(MetricsPusher::push);

        outcome
    }

    fn activate_awakened_tasks(wake: &WakeShared, active: &mut VecDeque<TaskRef>) {
        wake.drain_remote_wakes();

        // Remote notifications are converted to local notifications before inspecting either
        // activation path:
        // 1. receive an explicit wake signal via the `awakened` set (preferred, fast).
        // 2. probe embedded wake signals after local queue contention/capacity fallback (slower).
        //
        // Note that the same task may be awakened via both channels simultaneously, and that
        // spurious wake signals may be sent when the task is already active (the signal
        // may come from a caller who has no idea if the task is already awake or not).

        let first_awakened = active.len();
        wake.append_local_wakes(active);
        for task_ref in active.range(first_awakened..) {
            // SAFETY: Retirement removes every queued notification before task storage is
            // released, so every queued reference still targets a live task.
            unsafe { task_ref.as_task() }.clear_queued_notification();
            TASKS_ACTIVATED_VIA_AWAKENED_SET.with(Event::observe_once);
        }
    }

    fn poll_active_tasks(shared: &ExecutorShared, active: &mut VecDeque<TaskRef>, tasks: &mut TaskList) {
        let mut completed_count: usize = 0;
        let mut pending_count: usize = 0;

        while let Some(task_ref) = active.pop_front() {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };
            let pointer = task_ref.pointer();
            let linked = task.links().linked.get();

            if shared.is_shutting_down() {
                task.abort();
                shared.decrement_task_count(1);
                drop(tasks.remove(pointer));
                completed_count = completed_count.saturating_add(1);
                continue;
            }

            match task.poll() {
                task::Poll::Ready(()) => {
                    if linked {
                        drop(tasks.remove(pointer));
                    }
                    shared.decrement_task_count(1);
                    completed_count = completed_count.saturating_add(1);
                }
                task::Poll::Pending => {
                    pending_count = pending_count.saturating_add(1);
                    if !linked {
                        tasks.insert(task_ref);
                    }
                }
            }
        }

        Self::record_poll_metrics(pending_count, completed_count);
    }

    #[cfg_attr(test, mutants::skip)] // Just metrics reporting, not practical to test.
    fn record_poll_metrics(pending_count: usize, completed_count: usize) {
        if pending_count != 0 {
            TASKS_INACTIVATED.with(|x| x.observe(pending_count));
        }

        if completed_count != 0 {
            TASKS_COMPLETED.with(|x| x.observe(completed_count));
            TASKS_DROPPED.with(|x| x.observe(completed_count));
        }
    }

    /// Whether an immediately performed `execute_cycle()` would do any useful work.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Mutation can make the executor spin without useful work.
    fn has_work_to_do(shared: &ExecutorShared, active: &VecDeque<TaskRef>) -> bool {
        // Work for us means there is a task that we can poll (either because it is new
        // or because it has been awakened).

        // As it stands at time of writing, there does not seem to be a way for a task to enter
        // the "active" list during an execution cycle, so this should never happen. We assert it
        // here just in case we missed something and it does happen, or starts to happen in the
        // future after some refactoring (as we would then need to react to it).
        debug_assert!(active.is_empty(), "Active tasks should not be present at this point");

        shared.has_new_tasks() || shared.wake().has_local_wakes() || shared.wake().has_remote_wakes()
    }

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub(crate) fn begin_shutdown(&mut self) {
        self.shared.begin_shutdown();
        let aborted_count = Self::prepare_task_abort(&self.shared, &mut self.active, &mut self.tasks);
        Self::abort_prepared_tasks(&mut self.active, aborted_count, true);
    }

    pub(crate) fn owner_dropped(&mut self) {
        struct ClearRegistration(Arc<WakeShared>);

        impl Drop for ClearRegistration {
            fn drop(&mut self) {
                self.0.unregister_current();
            }
        }

        let _clear_registration = ClearRegistration(self.shared.wake_handle());
        self.shared.request_shutdown();

        let aborted_count = Self::prepare_task_abort(&self.shared, &mut self.active, &mut self.tasks);
        Self::abort_prepared_tasks(&mut self.active, aborted_count, false);
        if EXECUTOR_PUSHER.try_with(MetricsPusher::push).is_err() {
            // Executor cleanup must remain safe after metrics TLS has been destroyed.
        }
    }

    fn prepare_task_abort(shared: &ExecutorShared, active: &mut VecDeque<TaskRef>, tasks: &mut TaskList) -> usize {
        shared.wake().clear_local_wakes();

        for task_ref in active.iter() {
            drop(tasks.remove(task_ref.pointer()));
        }
        shared.drain_new_tasks(active);
        while let Some(task_ref) = tasks.pop_front() {
            active.push_back(task_ref);
        }
        let aborted_count = active.len();
        shared.decrement_task_count(aborted_count);
        match EXECUTOR_PUSHER.try_with(|_pusher| TASKS_ABORTED_ON_SHUTDOWN.try_with(|event| event.batch(aborted_count).observe_once())) {
            Ok(Ok(())) => {}
            Ok(Err(_access_error)) | Err(_access_error) => {
                // Metrics TLS may already be destroyed when an executor itself lives in TLS.
            }
        }

        aborted_count
    }

    fn abort_prepared_tasks(active: &mut VecDeque<TaskRef>, aborted_count: usize, record_drop_metric: bool) {
        while let Some(task_ref) = active.pop_front() {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            task.abort();
        }
        if record_drop_metric && aborted_count != 0 {
            TASKS_DROPPED.with(|x| x.observe(aborted_count));
        }
    }

    /// Returns whether we are at the end of a successful shutdown process.
    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    #[must_use]
    fn evaluate_shutdown_completion(shared: &ExecutorShared, active: &VecDeque<TaskRef>, tasks: &TaskList) -> bool {
        // During shutdown, no live task ownership remains in executor queues.
        debug_assert!(active.is_empty());
        debug_assert!(tasks.is_empty());

        shared.shutdown_complete()
    }
}

#[cfg_attr(test, mutants::skip)] // Executor::drop tests the same cleanup through owner_dropped().
impl Drop for ExecutorCore {
    fn drop(&mut self) {
        self.shared.request_shutdown();
        let aborted_count = Self::prepare_task_abort(&self.shared, &mut self.active, &mut self.tasks);
        Self::abort_prepared_tasks(&mut self.active, aborted_count, false);
        match EXECUTOR_PUSHER.try_with(MetricsPusher::push) {
            Ok(()) => {}
            Err(_access_error) => {
                // Executor cleanup must remain safe after metrics TLS has been destroyed.
            }
        }
        debug_assert!(self.shared.shutdown_complete());
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
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(ExecutorCore: Send, Sync);
    }
}
