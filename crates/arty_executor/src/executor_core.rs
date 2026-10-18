// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::task;
use std::thread::ThreadId;
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

impl ExecutorCore {
    /// Creates a new async task executor.
    ///
    /// The `owner_waker` is used by the executor to wake up its owner when more work has
    /// arrived for the executor and calling `execute_cycle()` is desirable.
    #[must_use]
    pub(crate) fn new(owner_waker: task::Waker, sink: Sink, thread_id: ThreadId) -> Self {
        Self {
            shared: ExecutorShared::new(owner_waker, sink, thread_id),
            active: VecDeque::new(),
            tasks: TaskList::default(),
            clock: SimpleClock::new_system().with_fast_instant(true),
            last_cycle_ended: None,
        }
    }

    pub(crate) fn wake(&self) -> &WakeShared {
        self.shared.wake()
    }

    pub(crate) fn shared(&self) -> &ExecutorShared {
        &self.shared
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.shared.has_tasks()
    }

    #[must_use]
    pub(crate) fn execute_cycle(&mut self) -> CycleOutcome {
        let _cycle = self.shared.wake().enter_cycle();
        let cycle_started = self.start_cycle();
        self.accept_new_tasks();
        self.activate_awakened_tasks();
        self.poll_active_tasks();
        self.cancel_if_shutting_down();
        let outcome = self.cycle_outcome();
        self.finish_cycle(cycle_started);
        outcome
    }

    fn start_cycle(&mut self) -> Instant {
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
        if accepted != 0 {
            TASKS_ACCEPTED.with(|event| event.observe(accepted));
        }
    }

    fn cancel_if_shutting_down(&mut self) {
        if self.shared.is_shutting_down() {
            let aborted_count = self.prepare_task_abort();
            self.abort_prepared_tasks(aborted_count, true);
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
        self.shared.wake().drain_remote_wakes();

        let first_awakened = self.active.len();
        self.shared.wake().append_local_wakes(&mut self.active);
        for task_ref in self.active.range(first_awakened..) {
            // SAFETY: Retirement removes every queued notification before task storage is
            // released, so every queued reference still targets a live task.
            unsafe { task_ref.as_task() }.clear_queued_notification();
            TASKS_ACTIVATED_VIA_AWAKENED_SET.with(Event::observe_once);
        }
    }

    fn poll_active_tasks(&mut self) {
        let mut completed_count: usize = 0;
        let mut pending_count: usize = 0;

        while let Some(task_ref) = self.active.pop_front() {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };
            let pointer = task_ref.pointer();
            let linked = task.links().linked.get();

            if self.shared.is_shutting_down() {
                task.abort();
                self.shared.decrement_task_count(1);
                drop(self.tasks.remove(pointer));
                completed_count = completed_count.saturating_add(1);
                continue;
            }

            match task.poll() {
                task::Poll::Ready(()) => {
                    if linked {
                        drop(self.tasks.remove(pointer));
                    }
                    self.shared.decrement_task_count(1);
                    completed_count = completed_count.saturating_add(1);
                }
                task::Poll::Pending => {
                    pending_count = pending_count.saturating_add(1);
                    if !linked {
                        self.tasks.insert(task_ref);
                    }
                }
            }
        }

        self.record_poll_metrics(pending_count, completed_count);
    }

    #[cfg_attr(test, mutants::skip)] // Just metrics reporting, not practical to test.
    fn record_poll_metrics(&self, pending_count: usize, completed_count: usize) {
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
    fn has_work_to_do(&self) -> bool {
        // Work for us means there is a task that we can poll (either because it is new
        // or because it has been awakened).

        // As it stands at time of writing, there does not seem to be a way for a task to enter
        // the "active" list during an execution cycle, so this should never happen. We assert it
        // here just in case we missed something and it does happen, or starts to happen in the
        // future after some refactoring (as we would then need to react to it).
        debug_assert!(self.active.is_empty(), "Active tasks should not be present at this point");

        self.shared.has_new_tasks() || self.shared.wake().has_local_wakes() || self.shared.wake().has_remote_wakes()
    }

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub(crate) fn begin_shutdown(&mut self) {
        self.shared.begin_shutdown();
        let aborted_count = self.prepare_task_abort();
        self.abort_prepared_tasks(aborted_count, true);
    }

    pub(crate) fn owner_dropped(&mut self) {
        self.shared.request_shutdown();

        let aborted_count = self.prepare_task_abort();
        self.abort_prepared_tasks(aborted_count, false);
        if EXECUTOR_PUSHER.try_with(MetricsPusher::push).is_err() {
            // Executor cleanup must remain safe after metrics TLS has been destroyed.
        }
    }

    fn prepare_task_abort(&mut self) -> usize {
        self.shared.wake().clear_local_wakes();
        for task_ref in &self.active {
            drop(self.tasks.remove(task_ref.pointer()));
        }
        self.shared.drain_new_tasks(&mut self.active);
        while let Some(task_ref) = self.tasks.pop_front() {
            self.active.push_back(task_ref);
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

    fn abort_prepared_tasks(&mut self, aborted_count: usize, record_drop_metric: bool) {
        while let Some(task_ref) = self.active.pop_front() {
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
    fn evaluate_shutdown_completion(&self) -> bool {
        // During shutdown, no live task ownership remains in executor queues.
        debug_assert!(self.active.is_empty());
        debug_assert!(self.tasks.is_empty());

        self.shared.shutdown_complete()
    }
}

#[cfg_attr(test, mutants::skip)] // Executor::drop tests the same cleanup through owner_dropped().
impl Drop for ExecutorCore {
    fn drop(&mut self) {
        self.shared.request_shutdown();
        let aborted_count = self.prepare_task_abort();
        self.abort_prepared_tasks(aborted_count, false);
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
