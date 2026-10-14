// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::{mem, task};

use nm::{Event, Magnitude, MetricsPusher, Push};
use plurality::MultiPool;
use tick::SimpleClock;

use crate::wake::WakeShared;
use crate::{BuildPointerHasher, CycleOutcome, ERR_POISONED_LOCK, JoinEventLake, JoinHandle, Task, TaskOptions, TaskRef, WakeSignal};

/// The real implementation of the executor, shared by different public "client" API surfaces.
///
/// * `Executor` is the public API used by the owner of the executor, the one who creates it,
///   controls its operation and is responsible for its shutdown orchestration.
/// * `TaskSet` is the public API used by anyone on the same thread to add tasks to the executor.
///   It can be freely cloned and shared, with its useful lifetime bounded by the lifetime of
///   `Executor` (after `Executor` is dropped, new submissions return a shutdown join error).
#[derive(Debug)]
pub(crate) struct ExecutorCore {
    reentrancy_safe: RefCell<ReentrancySafeState>,
    exclusive: RefCell<ExclusiveState>,
    wake: Arc<WakeShared>,

    _single_threaded: PhantomData<*const ()>,
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
/// This entire struct is `RefCell`-guarded and borrowed for the duration of `execute_cycle()`.
#[derive(Debug)]
struct ExclusiveState {
    /// All the tasks we want to poll.
    ///
    /// This is a `VecDeque` because we do not require set characteristics and a deque is faster.
    active: VecDeque<TaskRef>,

    /// The inactive set contains all the tasks that we consider to be in a state where polling
    /// will not make any progress. We will move them back to the active set after a waker notifies
    /// us that a future needs to be polled again because the poll may do something useful now.
    ///
    /// Note that the wake-up may arrive during the time the task is being polled. This does not
    /// mean it gets polled multiple times per cycle, though - we explicitly poll every task at
    /// most once per cycle, to avoid getting stuck on a hyperactive task.
    ///
    /// We use a `PointerHasher` for efficient hashing because we know via white-box knowledge
    /// that `TaskRef` is a thin wrapper around a pointer.
    inactive: HashSet<TaskRef, BuildPointerHasher>,

    /// Tasks that completed during this cycle and can be released after their independently pooled
    /// wake state has been retired.
    completed: VecDeque<TaskRef>,

    /// Used to measure the time spent during/between executor cycle processing and during shutdown.
    /// This is a very fast clock designed to be cheap to poll many times each millisecond (if we
    /// need to).
    clock: SimpleClock,

    // Used to report interval between cycles.
    last_cycle_ended: Option<Instant>,
}

// We prefer to get wake notifications via the "awakened" queue. This may not always be possible
// because the queue may be full or it may be locked (if the wake-up is coming from another thread).
//
// It is just a VecDeque so the size does not significantly impact wake-up processing time.
// Ideally, this queue should be at least as large as the number of I/O completions that we expect
// to handle per executor cycle, plus some additional capacity to handle any cross-thread events.
#[cfg(not(miri))]
pub(crate) const AWAKENED_CAPACITY: usize = 1024;

// Miri performance scales with memory access count, so large numbers are our enemy.
#[cfg(miri)]
pub(crate) const AWAKENED_CAPACITY: usize = 4;

#[expect(clippy::unused_self, reason = "semantically correct, even if not always necessary")]
impl ExecutorCore {
    /// Creates a new async task executor.
    ///
    /// The `owner_waker` is used by the executor to wake up its owner when more work has
    /// arrived for the executor and calling `execute_cycle()` is desirable.
    #[must_use]
    pub(crate) fn new(owner_waker: task::Waker) -> Self {
        Self {
            reentrancy_safe: RefCell::new(ReentrancySafeState {
                new_tasks: VecDeque::new(),
                task_storage: MultiPool::new(),
                join_events: JoinEventLake::new(),
                shutting_down: false,
            }),
            exclusive: RefCell::new(ExclusiveState {
                active: VecDeque::new(),
                inactive: HashSet::with_hasher(BuildPointerHasher::default()),
                completed: VecDeque::new(),
                clock: SimpleClock::new_system().with_fast_instant(true),
                last_cycle_ended: None,
            }),
            wake: Arc::new(WakeShared::new(
                Arc::new(Mutex::new(VecDeque::with_capacity(AWAKENED_CAPACITY))),
                Arc::new(AtomicBool::new(false)),
                owner_waker,
            )),
            _single_threaded: PhantomData,
        }
    }

    pub(crate) fn add_task<F, R>(&self, future: F, options: TaskOptions<R>) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        {
            let state = self.reentrancy_safe.borrow();
            if state.shutting_down {
                return state.join_events.disconnected();
            }
        }

        let future = future.into_future();
        let mut state = self.reentrancy_safe.borrow_mut();
        if state.shutting_down {
            return state.join_events.disconnected();
        }
        let (result_tx, join_handle) = state.join_events.rent::<R>();

        let task_ref = TaskRef::new(state.task_storage.alloc_box(Task::new_with_options(future, result_tx, options)));

        let wake_signal = WakeSignal::new(Arc::clone(&self.wake), task_ref);

        // SAFETY: The task is alive (we own it and just created it) and we are on the thread
        // where it was created (because we just created it). The executor is the only thing that
        // creates references to the tasks and it only ever creates temporary non-overlapping
        // references narrowly bounded to individual code blocks, ensuring that aliasing rules
        // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
        // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
        let task = unsafe { task_ref.as_task() };

        task.initialize(wake_signal);
        task.report_created();

        state.new_tasks.push_back(task_ref);

        join_handle
    }

    #[must_use]
    pub(crate) fn join_event_lake(&self) -> JoinEventLake {
        self.reentrancy_safe.borrow().join_events.clone()
    }

    #[must_use]
    pub(crate) fn has_tasks(&self) -> bool {
        !self.reentrancy_safe.borrow().task_storage.is_empty()
    }

    #[must_use]
    pub(crate) fn execute_cycle(&self) -> CycleOutcome {
        let mut state_exclusive = self.exclusive.borrow_mut();

        let cycle_start_timestamp = state_exclusive.clock.instant();

        if let Some(last_cycle_ended) = state_exclusive.last_cycle_ended {
            let cycle_gap = cycle_start_timestamp.saturating_duration_since(last_cycle_ended);
            CYCLE_GAP_MILLIS.with(|x| x.observe_millis(cycle_gap));
        }

        {
            let mut state_reentrant = self.reentrancy_safe.borrow_mut();

            self.accept_new_tasks(&mut state_exclusive, &mut state_reentrant);
            self.activate_awakened_tasks(&mut state_exclusive);
        }

        self.poll_active_tasks(&mut state_exclusive);

        {
            let state_reentrant = self.reentrancy_safe.borrow();

            Self::drop_completed_tasks(&mut state_exclusive, true);

            let outcome = if self.evaluate_shutdown_completion(&state_exclusive, &state_reentrant) {
                CycleOutcome::Shutdown
            } else if self.has_work_to_do(&state_exclusive, &state_reentrant) {
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
    fn accept_new_tasks(&self, state_exclusive: &mut ExclusiveState, state_reentrant: &mut ReentrancySafeState) {
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

    fn activate_awakened_tasks(&self, state_exclusive: &mut ExclusiveState) {
        // There are two ways to activate tasks:
        // 1. by receiving an explicit wake signal via the `awakened` set (preferred, fast).
        // 2. by probing the embedded wake signals (slower).
        //
        // Note that the same task may be awakened via both channels simultaneously, and that
        // spurious wake signals may be sent when the task is already active (the signal
        // may come from a caller who has no idea if the task is already awake or not).

        {
            // First, we simply drain the "awakened" queue, which is the preferred
            // way to wake up tasks. This is a fast TaskRef move, so the lock here is
            // hopefully short and mostly uncontended.
            let mut awakened = self.wake.awakened.lock().expect(ERR_POISONED_LOCK);

            if !awakened.is_empty() {
                // Process each contiguous slice in FIFO order, avoiding per-task ring-buffer
                // bookkeeping. TaskRef is Copy, so clearing once also avoids drain cleanup.
                for slice in <[_; 2]>::from(awakened.as_slices()) {
                    for &task_ref in slice {
                        // SAFETY: Retirement removes every queued notification before task
                        // storage is released, so every queued reference still targets a live task.
                        unsafe { task_ref.as_task() }.clear_queued_notification();
                        // It is theoretically possible for a completed task to be awakened, in which case
                        // we do nothing. We detect this by ensuring that the task was in the "inactive" set
                        // before we react to the wake notification. This also eliminates spurious wakes.
                        if state_exclusive.inactive.remove(&task_ref) {
                            state_exclusive.active.push_back(task_ref);

                            TASKS_ACTIVATED_VIA_AWAKENED_SET.with(Event::observe_once);
                        } else {
                            TASKS_ACTIVATED_SPURIOUS.with(Event::observe_once);
                        }
                    }
                }
                awakened.clear();
            }
        }

        // If we have been instructed to probe the embedded wake signals, we do so now.
        // This generally means the `awakened` queue was either full or locked, so could not
        // be used. It switches us into a less efficient mode here but what can you do - if
        // a lot of things are happening, someone has to pay the price.
        //
        // We use Acquire ordering as we are acquiring the synchronization block for the
        // wake-up flags inside the task wake signals.
        if !self.wake.probe_embedded_wake_signals.swap(false, atomic::Ordering::Acquire) {
            return;
        }

        // This means we need to inspect every task in `inactive` to see if it has
        // been awakened. This scales very poorly with large (1000+) task sets, which
        // is why we avoid it if at all possible.
        state_exclusive.inactive.retain(|task_ref| {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            if task.consume_awakened() {
                TASKS_ACTIVATED_VIA_PROBE.with(Event::observe_once);
                state_exclusive.active.push_back(*task_ref);
                false
            } else {
                true
            }
        });
    }

    fn poll_active_tasks(&self, state_exclusive: &mut ExclusiveState) {
        let completed_before = state_exclusive.completed.len();
        let inactive_before = state_exclusive.inactive.len();

        while let Some(task_ref) = state_exclusive.active.pop_front() {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            match task.poll() {
                task::Poll::Ready(()) => {
                    // The task has completed and retired its independently pooled wake state, so it
                    // can move to the completed list for release at the end of this cycle.
                    state_exclusive.completed.push_back(task_ref);
                }
                task::Poll::Pending => {
                    // The task is still pending, so we move it to the inactive set.
                    // It will return to the active set once something jiggles its waker.
                    state_exclusive.inactive.insert(task_ref);
                }
            }
        }

        let completed_diff = state_exclusive
            .completed
            .len()
            .checked_sub(completed_before)
            .expect("completed tasks set cannot decrease when completing more tasks");
        let inactive_diff = state_exclusive
            .inactive
            .len()
            .checked_sub(inactive_before)
            .expect("inactive tasks set cannot decrease when moving more tasks to inactive state");

        self.record_poll_metrics(inactive_diff, completed_diff);
    }

    #[cfg_attr(test, mutants::skip)] // Just metrics reporting, not practical to test.
    fn record_poll_metrics(&self, inactive_diff: usize, completed_diff: usize) {
        if inactive_diff != 0 {
            TASKS_INACTIVATED.with(|x| x.observe(inactive_diff));
        }

        if completed_diff != 0 {
            TASKS_COMPLETED.with(|x| x.observe(completed_diff));
        }
    }

    #[cfg_attr(test, mutants::skip)] // If tasks are not dropped, executor will never shut down, leading to infinite loop.
    fn drop_completed_tasks(state_exclusive: &mut ExclusiveState, record_cycle_metric: bool) {
        let completed_before = state_exclusive.completed.len();

        // We drop all completed tasks after their completion/cancellation path has retired any
        // independently held wake metadata.
        for task_ref in state_exclusive.completed.drain(..) {
            // SAFETY: Draining removes each completed task exactly once, after its wake metadata
            // has been retired by completion or cancellation.
            unsafe { task_ref.release() };
        }

        if record_cycle_metric && completed_before > 0 {
            TASKS_DROPPED.with(|x| x.observe(completed_before));
        }
    }

    /// Whether an immediately performed `execute_cycle()` would do any useful work.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Mutation can make the executor spin without useful work.
    fn has_work_to_do(&self, state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) -> bool {
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

        !state_reentrant.new_tasks.is_empty()
            || !self.wake.awakened.lock().expect(ERR_POISONED_LOCK).is_empty()
            || self.wake.probe_embedded_wake_signals.load(atomic::Ordering::Relaxed)
    }

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub(crate) fn begin_shutdown(&self) {
        let mut state_exclusive = self.exclusive.borrow_mut();
        let mut state_reentrant = self.reentrancy_safe.borrow_mut();

        assert!(
            !state_reentrant.shutting_down,
            "Cannot start shutdown process when it has already been started"
        );
        state_reentrant.shutting_down = true;

        let aborted_count = Self::prepare_task_abort(&self.wake, &mut state_exclusive, &mut state_reentrant);
        drop(state_reentrant);
        Self::abort_prepared_tasks(&state_exclusive, aborted_count);
    }

    fn prepare_task_abort(wake: &WakeShared, state_exclusive: &mut ExclusiveState, state_reentrant: &mut ReentrancySafeState) -> usize {
        let mut awakened = wake.awakened.lock().expect(ERR_POISONED_LOCK);
        for &task_ref in awakened.iter() {
            // SAFETY: Shutdown has not released task storage yet; every queued reference remains live.
            unsafe { task_ref.as_task() }.clear_queued_notification();
        }
        awakened.clear();
        drop(awakened);

        // Needed for split borrowing.
        let state_exclusive_real: &mut ExclusiveState = state_exclusive;

        let new = &mut state_reentrant.new_tasks;
        let active = &mut state_exclusive_real.active;
        let inactive = &mut state_exclusive_real.inactive;
        let completed = &mut state_exclusive_real.completed;

        let aborted_count = new.len().saturating_add(active.len().saturating_add(inactive.len()));
        match EXECUTOR_PUSHER.try_with(|_pusher| TASKS_ABORTED_ON_SHUTDOWN.try_with(|event| event.batch(aborted_count).observe_once())) {
            Ok(Ok(())) => {}
            Ok(Err(_access_error)) | Err(_access_error) => {
                // Metrics TLS may already be destroyed when an executor itself lives in TLS.
            }
        }

        for task_ref in active.drain(..).chain(inactive.drain()).chain(new.drain(..)) {
            completed.push_back(task_ref);
        }

        aborted_count
    }

    fn abort_prepared_tasks(state_exclusive: &ExclusiveState, aborted_count: usize) {
        let abort_start = state_exclusive
            .completed
            .len()
            .checked_sub(aborted_count)
            .expect("prepared task count cannot exceed completed task count");
        for &task_ref in state_exclusive.completed.range(abort_start..) {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            task.abort();
        }
    }

    /// Returns whether we are at the end of a successful shutdown process.
    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    #[must_use]
    fn evaluate_shutdown_completion(&self, state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) -> bool {
        if !state_reentrant.shutting_down {
            // We are not in a shutdown process.
            return false;
        }

        // During shutdown, tasks can only exist in the "completed" state.
        debug_assert!(state_exclusive.active.is_empty());
        debug_assert!(state_reentrant.new_tasks.is_empty());
        debug_assert!(state_exclusive.inactive.is_empty());

        state_exclusive.completed.is_empty() && state_reentrant.task_storage.is_empty()
    }
}

impl Drop for ExecutorCore {
    fn drop(&mut self) {
        let state_exclusive = self.exclusive.get_mut();
        let state_reentrant = self.reentrancy_safe.get_mut();

        if !state_reentrant.shutting_down {
            state_reentrant.shutting_down = true;
            let aborted_count = Self::prepare_task_abort(&self.wake, state_exclusive, state_reentrant);
            Self::abort_prepared_tasks(state_exclusive, aborted_count);
        }

        Self::drop_completed_tasks(state_exclusive, false);
        match EXECUTOR_PUSHER.try_with(MetricsPusher::push) {
            Ok(()) => {}
            Err(_access_error) => {
                // Executor cleanup must remain safe after metrics TLS has been destroyed.
            }
        }
        debug_assert!(state_reentrant.task_storage.is_empty());
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

    static TASKS_ACTIVATED_VIA_PROBE: Event<Push> = Event::builder()
        .name("ae_tasks_activated_via_probe")
        .pusher_local(&EXECUTOR_PUSHER)
        .build();

    static TASKS_ACTIVATED_SPURIOUS: Event<Push> = Event::builder()
        .name("ae_tasks_activated_spurious")
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
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::pin::Pin;
    use std::rc::Rc;

    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::TypeErasedTask;

    #[derive(Clone, Copy)]
    enum TaskLocation {
        New,
        Active,
        Inactive,
        Completed,
    }

    struct SetSignalOnDrop(Rc<Cell<bool>>);

    impl Drop for SetSignalOnDrop {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    struct PanicOnDrop(Rc<Cell<bool>>);

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            self.0.set(true);
            panic!("panic from dirty-drop task destructor");
        }
    }

    /// Synthetic task that isolates dirty-teardown ownership from the task/waker lifecycle.
    ///
    /// `inert` controls the reclamation decision; every other lifecycle operation is outside this
    /// fixture's boundary and therefore rejects calls.
    struct DirtyDropTask {
        _signal: SetSignalOnDrop,
        inert: bool,
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    impl TypeErasedTask for DirtyDropTask {
        fn poll(self: Pin<&Self>) -> task::Poll<()> {
            unreachable!("dirty-drop fixture is never polled")
        }

        fn is_inert(&self) -> bool {
            self.inert
        }

        fn consume_awakened(&self) -> bool {
            unreachable!("dirty-drop fixture is never awakened")
        }

        fn abort(self: Pin<&Self>) {
            unreachable!("dirty-drop fixture is never aborted")
        }

        unsafe fn initialize(self: Pin<&Self>, _wake_signal: WakeSignal) {
            unreachable!("dirty-drop fixture is never initialized")
        }

        #[cfg(debug_assertions)]
        fn inspect_waker_backtraces(&self, _f: &mut dyn FnMut(&Backtrace)) {
            unreachable!("dirty-drop fixture is never inspected")
        }
    }

    struct PanicOnDropTask {
        _panic_on_drop: PanicOnDrop,
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    impl TypeErasedTask for PanicOnDropTask {
        fn poll(self: Pin<&Self>) -> task::Poll<()> {
            unreachable!("panicking-drop fixture is never polled")
        }

        fn is_inert(&self) -> bool {
            unreachable!("panicking-drop fixture is never inspected")
        }

        fn consume_awakened(&self) -> bool {
            unreachable!("panicking-drop fixture is never awakened")
        }

        fn abort(self: Pin<&Self>) {
            unreachable!("panicking-drop fixture is never aborted")
        }

        unsafe fn initialize(self: Pin<&Self>, _wake_signal: WakeSignal) {
            unreachable!("panicking-drop fixture is never initialized")
        }

        #[cfg(debug_assertions)]
        fn inspect_waker_backtraces(&self, _f: &mut dyn FnMut(&Backtrace)) {
            unreachable!("panicking-drop fixture is never inspected")
        }
    }

    /// Inserts a raw pooled task directly into one scheduler collection, without normal
    /// registration or wake initialization. The returned `TaskRef` lets the non-inert case clean
    /// up its deliberately retained raw owner after observing executor teardown.
    fn dirty_drop_fixture(location: TaskLocation, inert: bool) -> (ExecutorCore, TaskRef, Rc<Cell<bool>>) {
        let dropped = Rc::new(Cell::new(false));

        // SAFETY: This test intentionally violates the graceful-shutdown requirement to verify
        // dirty-teardown ownership behavior before the programming error is reported.
        let executor = unsafe { ExecutorCore::new(task::Waker::noop().clone(), Duration::ZERO, ShutdownTimeoutBehavior::Panic) };
        let task = executor.reentrancy_safe.borrow().task_storage.alloc_box(DirtyDropTask {
            _signal: SetSignalOnDrop(Rc::clone(&dropped)),
            inert,
        });
        let task_ref = TaskRef::new(task);

        match location {
            TaskLocation::New => executor.reentrancy_safe.borrow_mut().new_tasks.push_back(task_ref),
            TaskLocation::Active => executor.exclusive.borrow_mut().active.push_back(task_ref),
            TaskLocation::Inactive => {
                executor.exclusive.borrow_mut().inactive.insert(task_ref);
            }
            TaskLocation::Completed => executor.exclusive.borrow_mut().completed.push_back(task_ref),
        }

        (executor, task_ref, dropped)
    }

    fn assert_dirty_drop_reclaims_task(location: TaskLocation) {
        let (executor, _task_ref, dropped) = dirty_drop_fixture(location, true);

        let result = catch_unwind(AssertUnwindSafe(|| drop(executor)));

        assert!(result.is_err());
        assert!(dropped.get());
    }

    #[test]
    fn dirty_drop_reclaims_new_task() {
        assert_dirty_drop_reclaims_task(TaskLocation::New);
    }

    #[test]
    fn dirty_drop_reclaims_active_task() {
        assert_dirty_drop_reclaims_task(TaskLocation::Active);
    }

    #[test]
    fn dirty_drop_reclaims_inactive_task() {
        assert_dirty_drop_reclaims_task(TaskLocation::Inactive);
    }

    #[test]
    fn dirty_drop_reclaims_completed_task() {
        assert_dirty_drop_reclaims_task(TaskLocation::Completed);
    }

    #[test]
    fn dirty_drop_retains_non_inert_task() {
        let (executor, task_ref, dropped) = dirty_drop_fixture(TaskLocation::Inactive, false);

        let result = catch_unwind(AssertUnwindSafe(|| drop(executor)));

        assert!(result.is_err());
        assert!(!dropped.get());

        // SAFETY: Dirty teardown retained this unique raw owner, and the fixture has no external
        // references. Reconstructing it exactly once prevents the test itself from leaking.
        unsafe { task_ref.release() };
        assert!(dropped.get());
    }

    #[test]
    fn unwind_retains_task_whose_destructor_panics() {
        let dropped = Rc::new(Cell::new(false));

        // SAFETY: The executor is deliberately dropped during an existing unwind to verify that
        // its double-panic guard retains raw task owners instead of running their destructors.
        let executor = unsafe { ExecutorCore::new(task::Waker::noop().clone(), Duration::ZERO, ShutdownTimeoutBehavior::Panic) };
        let task = executor.reentrancy_safe.borrow().task_storage.alloc_box(PanicOnDropTask {
            _panic_on_drop: PanicOnDrop(Rc::clone(&dropped)),
        });
        let task_ref = TaskRef::new(task);
        executor.reentrancy_safe.borrow_mut().new_tasks.push_back(task_ref);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _executor = executor;
            panic!("original panic");
        }))
        .unwrap_err();

        assert_eq!(panic.downcast_ref::<&str>(), Some(&"original panic"));
        assert!(!dropped.get());

        // SAFETY: The unwind path deliberately retained this unique raw owner. The fixture has no
        // external references, so reconstructing it exactly once is valid; its expected destructor
        // panic is contained by this test.
        let release = catch_unwind(AssertUnwindSafe(|| unsafe { task_ref.release() }));
        assert!(release.is_err());
        assert!(dropped.get());
    }

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(ExecutorCore: Send, Sync);
    }
}
