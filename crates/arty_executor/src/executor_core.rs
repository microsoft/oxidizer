// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use std::{mem, task};

use nm::{Event, Magnitude, MetricsPusher, Push};
use observed::Sink;
use plurality::MultiPool;
use tick::SimpleClock;

use crate::remote::Sender as RemoteSender;
use crate::wake::WakeShared;
use crate::{CycleOutcome, JoinEventLake, JoinHandle, Task, TaskList, TaskOptions, TaskRef, TypeErasedTask, WakeSignal};

/// Owner-only executor state.
#[derive(Debug)]
pub(crate) struct ExecutorCore {
    shared: Rc<ExecutorShared>,
    exclusive: ExclusiveState,
}

/// Reentrant state shared with [`TaskSet`](crate::TaskSet).
#[derive(Debug)]
pub(crate) struct ExecutorShared {
    reentrancy_safe: RefCell<ReentrancySafeState>,
    wake: Arc<WakeShared>,
    sink: Sink,
}

/// This portion of the executor state is reentrancy-safe - during `execute_cycle()`, external code
/// may call back into the executor and call methods that depend on this state. Essentially, this is
/// the data set required to enqueue new tasks.
///
/// This entire struct is `RefCell`-guarded and borrowed for the duration of each individual
/// non-overlapping time span during which its contents may be used. In other words, whenever we are
/// calling into external code in `execute_cycle()` we must release the borrow.
#[derive(Debug)]
struct ReentrancySafeState {
    /// The new set contains all the tasks that have been created but not processed as part of
    /// any `execute_cycle()` call yet.
    ///
    /// This is a `VecDeque` because we do not require set characteristics and a deque is faster.
    new_tasks: VecDeque<TaskRef>,

    /// Storage for all the tasks registered with the executor. Once a task has been registered,
    /// the executor only sees it as a [`TypeErasedTask`][crate::TypeErasedTask], with only the
    /// task itself knowing the specific type of the future and the type of the result it produces.
    task_storage: MultiPool,

    /// Logical tasks whose payload has not completed or been cancelled.
    ///
    /// Retained inert wakers may keep an empty pooled allocation alive after this reaches zero.
    task_count: usize,

    /// Storage for result channels. Managed endpoints retain detached pool slots independently,
    /// allowing join handles to outlive the executor safely.
    join_events: JoinEventLake,

    /// In shutdown mode, all tasks are cancelled and retired. New submissions produce a
    /// shutdown join error without registering a task.
    shutting_down: bool,
}

/// This portion of the executor state is exclusive to its internal use - even if something calls
/// back into the executor during `execute_cycle()`, this state is never touched.
///
/// This state is directly owned and mutably borrowed for the duration of `execute_cycle()`.
#[derive(Debug)]
struct ExclusiveState {
    /// All the tasks we want to poll.
    ///
    /// This is a `VecDeque` because we do not require set characteristics and a deque is faster.
    active: VecDeque<TaskRef>,

    /// Stable owner-thread inventory of pending tasks.
    tasks: TaskList,

    /// Used to measure the time spent during/between executor cycle processing and during shutdown.
    /// This is a very fast clock designed to be cheap to poll many times each millisecond (if we
    /// need to).
    clock: SimpleClock,

    // Used to report interval between cycles.
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
        let shared = Rc::new(ExecutorShared {
            reentrancy_safe: RefCell::new(ReentrancySafeState {
                new_tasks: VecDeque::new(),
                task_storage: MultiPool::new(),
                task_count: 0,
                join_events: JoinEventLake::new(),
                shutting_down: false,
            }),
            wake: Arc::new(WakeShared::new(owner_waker)),
            sink,
        });
        Self {
            shared,
            exclusive: ExclusiveState {
                active: VecDeque::new(),
                tasks: TaskList::default(),
                clock: SimpleClock::new_system().with_fast_instant(true),
                last_cycle_ended: None,
            },
        }
    }

    pub(crate) fn register_current(&self) {
        self.shared.wake.register_current();
    }

    pub(crate) fn shared(&self) -> &Rc<ExecutorShared> {
        &self.shared
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.shared.has_tasks()
    }
}

impl ExecutorShared {
    pub(crate) fn add_task<F, R>(&self, future: F, options: TaskOptions) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        if let Some(join_handle) = self.rejected_submission() {
            return join_handle;
        }
        let future = future.into_future();
        self.register_task(
            move |join_events, sink| {
                let (result_tx, join_handle) = join_events.rent();
                (Task::new_with_options(future, result_tx, options, sink), join_handle)
            },
            JoinEventLake::disconnected,
        )
    }

    pub(crate) fn add_remote_task<F, R>(&self, future: F, options: TaskOptions, remote_result_tx: RemoteSender<R>)
    where
        F: IntoFuture<Output = R> + 'static,
        R: Send + 'static,
    {
        if self.reentrancy_safe.borrow().shutting_down {
            return;
        }
        let future = future.into_future();
        self.register_task(
            move |_join_events, sink| (Task::new_remote(future, options, sink, remote_result_tx), ()),
            |_join_events| (),
        );
    }

    fn rejected_submission<R: 'static>(&self) -> Option<JoinHandle<R>> {
        let state = self.reentrancy_safe.borrow();
        state.shutting_down.then(|| state.join_events.disconnected())
    }

    fn register_task<O, T>(&self, create: impl FnOnce(&JoinEventLake, Sink) -> (T, O), reject: impl FnOnce(&JoinEventLake) -> O) -> O
    where
        T: TypeErasedTask + 'static,
    {
        let (reporter, output) = {
            let mut state = self.reentrancy_safe.borrow_mut();
            if state.shutting_down {
                return reject(&state.join_events);
            }
            let (task, output) = create(&state.join_events, self.sink.clone());

            let task_ref = TaskRef::new(state.task_storage.alloc_arc_pin(task));

            let wake_signal = WakeSignal::new(Arc::clone(&self.wake), task_ref.pointer());

            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            task.initialize(wake_signal);
            let reporter = task.created_reporter();
            state.new_tasks.push_back(task_ref);
            state.task_count = state.task_count.checked_add(1).expect("live task count cannot overflow");
            (reporter, output)
        };

        // Telemetry processors are external code and may submit another task. The state borrow
        // must be released before this call.
        if let Some(reporter) = reporter {
            reporter.report();
        }
        output
    }

    #[must_use]
    pub(crate) fn join_event_lake(&self) -> JoinEventLake {
        self.reentrancy_safe.borrow().join_events.clone()
    }

    #[must_use]
    pub(crate) fn has_tasks(&self) -> bool {
        self.reentrancy_safe.borrow().task_count != 0
    }

    pub(crate) fn request_shutdown(&self) {
        self.reentrancy_safe.borrow_mut().shutting_down = true;
    }
}

impl ExecutorCore {
    #[must_use]
    pub(crate) fn execute_cycle(&mut self) -> CycleOutcome {
        let shared = &self.shared;
        let _cycle = shared.wake.enter_cycle();
        let state_exclusive = &mut self.exclusive;

        let cycle_start_timestamp = state_exclusive.clock.instant();

        if let Some(last_cycle_ended) = state_exclusive.last_cycle_ended {
            let cycle_gap = cycle_start_timestamp.saturating_duration_since(last_cycle_ended);
            CYCLE_GAP_MILLIS.with(|x| x.observe_millis(cycle_gap));
        }

        {
            let mut state_reentrant = shared.reentrancy_safe.borrow_mut();
            Self::accept_new_tasks(state_exclusive, &mut state_reentrant);
        }
        Self::activate_awakened_tasks(&shared.wake, state_exclusive);

        Self::poll_active_tasks(shared, state_exclusive);

        if shared.reentrancy_safe.borrow().shutting_down {
            let aborted_count = {
                let mut state_reentrant = shared.reentrancy_safe.borrow_mut();
                Self::prepare_task_abort(&shared.wake, state_exclusive, &mut state_reentrant)
            };
            Self::abort_prepared_tasks(state_exclusive, aborted_count, true);
        }

        {
            let state_reentrant = shared.reentrancy_safe.borrow();

            let outcome = if Self::evaluate_shutdown_completion(state_exclusive, &state_reentrant) {
                CycleOutcome::Shutdown
            } else if Self::has_work_to_do(&shared.wake, state_exclusive, &state_reentrant) {
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

            let cycle_end_timestamp = state_exclusive.clock.instant();

            let cycle_duration = cycle_end_timestamp.saturating_duration_since(cycle_start_timestamp);
            state_exclusive.last_cycle_ended = Some(cycle_end_timestamp);
            CYCLE_DURATION_MILLIS.with(|x| x.observe_millis(cycle_duration));

            // Publish metrics from this cycle.
            EXECUTOR_PUSHER.with(MetricsPusher::push);

            outcome
        }
    }

    /// Accepts new tasks that have been registered with the executor since the last call to this
    /// method.
    fn accept_new_tasks(state_exclusive: &mut ExclusiveState, state_reentrant: &mut ReentrancySafeState) {
        if state_reentrant.new_tasks.is_empty() {
            // Nothing to do, no new tasks.
            return;
        }

        TASKS_ACCEPTED.with(|x| x.observe(state_reentrant.new_tasks.len()));

        // Every cycle drains the active queue. Swap its reusable buffer with the new-task queue
        // instead of copying each task, leaving a separate queue for reentrant additions.
        debug_assert!(
            state_exclusive.active.is_empty(),
            "active tasks are drained at the end of each cycle"
        );
        mem::swap(&mut state_exclusive.active, &mut state_reentrant.new_tasks);
    }

    fn activate_awakened_tasks(wake: &WakeShared, state_exclusive: &mut ExclusiveState) {
        wake.drain_remote_wakes();

        // Remote notifications are converted to local notifications before inspecting either
        // activation path:
        // 1. receive an explicit wake signal via the `awakened` set (preferred, fast).
        // 2. probe embedded wake signals after local queue contention/capacity fallback (slower).
        //
        // Note that the same task may be awakened via both channels simultaneously, and that
        // spurious wake signals may be sent when the task is already active (the signal
        // may come from a caller who has no idea if the task is already awake or not).

        let first_awakened = state_exclusive.active.len();
        wake.append_local_wakes(&mut state_exclusive.active);
        for task_ref in state_exclusive.active.range(first_awakened..) {
            // SAFETY: Retirement removes every queued notification before task storage is
            // released, so every queued reference still targets a live task.
            unsafe { task_ref.as_task() }.clear_queued_notification();
            TASKS_ACTIVATED_VIA_AWAKENED_SET.with(Event::observe_once);
        }
    }

    fn poll_active_tasks(shared: &ExecutorShared, state_exclusive: &mut ExclusiveState) {
        let mut completed_count: usize = 0;
        let mut pending_count: usize = 0;

        while let Some(task_ref) = state_exclusive.active.pop_front() {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };
            let pointer = task_ref.pointer();
            let linked = task.links().linked.get();

            if shared.reentrancy_safe.borrow().shutting_down {
                task.abort();
                Self::decrement_task_count(shared, 1);
                drop(state_exclusive.tasks.remove(pointer));
                completed_count = completed_count.saturating_add(1);
                continue;
            }

            match task.poll() {
                task::Poll::Ready(()) => {
                    if linked {
                        drop(state_exclusive.tasks.remove(pointer));
                    }
                    Self::decrement_task_count(shared, 1);
                    completed_count = completed_count.saturating_add(1);
                }
                task::Poll::Pending => {
                    pending_count = pending_count.saturating_add(1);
                    if !linked {
                        state_exclusive.tasks.insert(task_ref);
                    }
                }
            }
        }

        Self::record_poll_metrics(pending_count, completed_count);
    }

    fn decrement_task_count(shared: &ExecutorShared, count: usize) {
        let mut state = shared.reentrancy_safe.borrow_mut();
        state.task_count = state
            .task_count
            .checked_sub(count)
            .expect("completed task count cannot exceed live task count");
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
    fn has_work_to_do(wake: &WakeShared, state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) -> bool {
        // Work for us means there is a task that we can poll (either because it is new
        // or because it has been awakened).

        // As it stands at time of writing, there does not seem to be a way for a task to enter
        // the "active" list during an execution cycle, so this should never happen. We assert it
        // here just in case we missed something and it does happen, or starts to happen in the
        // future after some refactoring (as we would then need to react to it).
        debug_assert!(
            state_exclusive.active.is_empty(),
            "Active tasks should not be present at this point"
        );

        !state_reentrant.new_tasks.is_empty() || wake.has_local_wakes() || wake.has_remote_wakes()
    }

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub(crate) fn begin_shutdown(&mut self) {
        {
            let mut state = self.shared.reentrancy_safe.borrow_mut();
            assert!(
                !state.shutting_down,
                "Cannot start shutdown process when it has already been started"
            );
            state.shutting_down = true;
        }
        let state_exclusive = &mut self.exclusive;
        let mut state_reentrant = self.shared.reentrancy_safe.borrow_mut();

        let aborted_count = Self::prepare_task_abort(&self.shared.wake, state_exclusive, &mut state_reentrant);
        drop(state_reentrant);
        Self::abort_prepared_tasks(state_exclusive, aborted_count, true);
    }

    pub(crate) fn owner_dropped(&mut self) {
        struct ClearRegistration(Arc<WakeShared>);

        impl Drop for ClearRegistration {
            fn drop(&mut self) {
                self.0.unregister_current();
            }
        }

        let _clear_registration = ClearRegistration(Arc::clone(&self.shared.wake));
        self.shared.request_shutdown();

        let state_exclusive = &mut self.exclusive;
        let mut state_reentrant = self.shared.reentrancy_safe.borrow_mut();
        let aborted_count = Self::prepare_task_abort(&self.shared.wake, state_exclusive, &mut state_reentrant);
        drop(state_reentrant);
        Self::abort_prepared_tasks(state_exclusive, aborted_count, false);
        if EXECUTOR_PUSHER.try_with(MetricsPusher::push).is_err() {
            // Executor cleanup must remain safe after metrics TLS has been destroyed.
        }
    }

    fn prepare_task_abort(wake: &WakeShared, state_exclusive: &mut ExclusiveState, state_reentrant: &mut ReentrancySafeState) -> usize {
        wake.clear_local_wakes();

        let new = &mut state_reentrant.new_tasks;
        let active = &mut state_exclusive.active;
        let tasks = &mut state_exclusive.tasks;

        for task_ref in active.iter() {
            drop(tasks.remove(task_ref.pointer()));
        }
        active.extend(new.drain(..));
        while let Some(task_ref) = tasks.pop_front() {
            active.push_back(task_ref);
        }
        let aborted_count = active.len();
        state_reentrant.task_count = state_reentrant
            .task_count
            .checked_sub(aborted_count)
            .expect("aborted task count cannot exceed live task count");
        match EXECUTOR_PUSHER.try_with(|_pusher| TASKS_ABORTED_ON_SHUTDOWN.try_with(|event| event.batch(aborted_count).observe_once())) {
            Ok(Ok(())) => {}
            Ok(Err(_access_error)) | Err(_access_error) => {
                // Metrics TLS may already be destroyed when an executor itself lives in TLS.
            }
        }

        aborted_count
    }

    fn abort_prepared_tasks(state_exclusive: &mut ExclusiveState, aborted_count: usize, record_drop_metric: bool) {
        while let Some(task_ref) = state_exclusive.active.pop_front() {
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
    fn evaluate_shutdown_completion(state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) -> bool {
        if !state_reentrant.shutting_down {
            // We are not in a shutdown process.
            return false;
        }

        // During shutdown, no live task ownership remains in executor queues.
        debug_assert!(state_exclusive.active.is_empty());
        debug_assert!(state_reentrant.new_tasks.is_empty());
        debug_assert!(state_exclusive.tasks.is_empty());

        state_reentrant.task_count == 0
    }
}

#[cfg_attr(test, mutants::skip)] // Executor::drop tests the same cleanup through owner_dropped().
impl Drop for ExecutorCore {
    fn drop(&mut self) {
        let state_exclusive = &mut self.exclusive;
        let mut state_reentrant = self.shared.reentrancy_safe.borrow_mut();

        state_reentrant.shutting_down = true;
        let aborted_count = Self::prepare_task_abort(&self.shared.wake, state_exclusive, &mut state_reentrant);
        Self::abort_prepared_tasks(state_exclusive, aborted_count, false);
        match EXECUTOR_PUSHER.try_with(MetricsPusher::push) {
            Ok(()) => {}
            Err(_access_error) => {
                // Executor cleanup must remain safe after metrics TLS has been destroyed.
            }
        }
        debug_assert_eq!(state_reentrant.task_count, 0);
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
