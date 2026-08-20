// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{task, thread};

use events_once::RawLocalEventLake;
use infinity_pool::{DropPolicy, RawBlindPool};
use nm::{Event, Magnitude, MetricsPusher, Push};
use tick::SimpleClock;

use crate::{
    BuildPointerHasher, CycleOutcome, ERR_POISONED_LOCK, JoinHandle, RawPooledCastTypeErasedTask, ShutdownTimeoutBehavior, Task, TaskRef,
    WakeSignal,
};

/// The real implementation of the executor, shared by different public "client" API surfaces.
///
/// * `Executor` is the public API used by the owner of the executor, the one who creates it,
///   controls its operation and is responsible for its shutdown orchestration.
/// * `TaskSet` is the public API used by anyone on the same thread to add tasks to the executor.
///   It can be freely cloned and shared, with its useful lifetime bounded by the lifetime of
///   `Executor` (after `Executor` is dropped, a `TaskSet` will simply panic when accessed).
#[derive(Debug)]
pub(crate) struct ExecutorCore {
    reentrancy_safe: RefCell<ReentrancySafeState>,
    exclusive: RefCell<ExclusiveState>,
    shared: SharedState,

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

    /// Storage for the channels used to deliver results from tasks to join handles. We use pooled
    /// event storage, which requires us to only drop this object when we identify that it is safe
    /// to do so (via `.is_empty()`). This means that all join handles must be dropped first.
    ///
    /// Optimization?: we could potentially benefit from embedding the result channels directly into
    /// the tasks to further reduce object sprawl (to have one object that contains all data related
    /// to one task) and co-locate the data. A downside might be that tasks could remain non-inert
    /// for longer with this model, causing executor cycle execution to become less efficient.
    result_events: RawLocalEventLake,

    /// Storage for all the tasks registered with the executor. Once a task has been registered,
    /// the executor only sees it as a [`TypeErasedTask`][crate::TypeErasedTask], with only the
    /// task itself knowing the specific type of the future and the type of the result it produces.
    task_storage: RawBlindPool,

    /// In shutdown mode (`Some`), all tasks are considered completed and the only thing we do is
    /// wait for them to become inert (which may be driven by uncontrollable actions of foreign
    /// threads). New tasks can no longer be scheduled in this mode (a panic will occur). If the
    /// deadline is reached without a successful shutdown, we terminate the process and try to report
    /// the underlying reasons.
    shutdown_deadline: Option<Instant>,
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

    /// These tasks have completed and we are waiting for them to become inert before we can release
    /// their resources. Tasks can sit here forever, although that suggests some resource leak and
    /// will degrade executor cycle performance. These tasks block the shutdown process.
    completed: VecDeque<TaskRef>,

    /// Used to measure the time spent during/between executor cycle processing and during shutdown.
    /// This is a very fast clock designed to be cheap to poll many times each millisecond (if we
    /// need to).
    clock: SimpleClock,

    // Used to report interval between cycles.
    last_cycle_ended: Option<Instant>,
}

/// This portion of the executor state is of a form that allows it to be naturally shared between
/// different users (e.g. mutex-protected or immutable), so it needs no special handling.
///
/// We could just put all this stuff into `Executor` itself but let's follow the pattern above.
#[derive(Debug)]
struct SharedState {
    /// The primary mechanism used to signal that a task has awoken and needs to be moved from the
    /// inactive queue to the active queue. We ONLY add entries to this list if we can do so without
    /// waiting on the lock, to minimize time we spend blocked on cross-thread synchronization. We
    /// also only add entries if we do not need to increase the capacity, to avoid allocating the new
    /// data structure on a different thread from the consuming thread (and therefore potentially in
    /// a different memory region, which would lead to inefficiency). If an entry cannot be added to
    /// this queue for any reason, the `probe_embedded_wake_signals` is set instead and the next cycle
    /// of the executor will probe the awakened status of every inactive task.
    ///
    /// This is a `VecDeque` because we need to be able to preallocate the capacity (insertions are
    /// always allocation-free because they may come from a different thread, so we cannot allocate).
    awakened: Arc<Mutex<VecDeque<TaskRef>>>,

    /// When a waker cannot lock the `awakened` queue or when the queue is full, it will set this
    /// flag to indicate that the awakened status of every inactive task should be directly probed.
    probe_embedded_wake_signals: Arc<AtomicBool>,

    /// This is used to wake up the owner of the executor when more work has
    /// been enqueued for the executor and calling `execute_cycle()` is desirable.
    owner_waker: task::Waker,

    /// If the executor fails to shut down after this much time has passed from the start of the
    /// shutdown process, we panic and report whatever debug information we have available.
    shutdown_timeout: Duration,
    shutdown_timeout_behavior: ShutdownTimeoutBehavior,
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
    ///
    /// # Safety
    ///
    /// The returned object must not be dropped until a call to
    /// [`execute_cycle()`][Self::execute_cycle] returns [`CycleOutcome::Shutdown`].
    #[must_use]
    pub(crate) unsafe fn new(
        owner_waker: task::Waker,
        shutdown_timeout: Duration,
        shutdown_timeout_behavior: ShutdownTimeoutBehavior,
    ) -> Self {
        Self {
            reentrancy_safe: RefCell::new(ReentrancySafeState {
                new_tasks: VecDeque::new(),
                result_events: RawLocalEventLake::new(),
                task_storage: RawBlindPool::builder().drop_policy(DropPolicy::MustNotDropContents).build(),
                shutdown_deadline: None,
            }),
            exclusive: RefCell::new(ExclusiveState {
                active: VecDeque::new(),
                inactive: HashSet::with_hasher(BuildPointerHasher::default()),
                completed: VecDeque::new(),
                clock: SimpleClock::new_system().with_fast_instant(true),
                last_cycle_ended: None,
            }),
            shared: SharedState {
                awakened: Arc::new(Mutex::new(VecDeque::with_capacity(AWAKENED_CAPACITY))),
                probe_embedded_wake_signals: Arc::new(AtomicBool::new(false)),
                owner_waker,
                shutdown_timeout,
                shutdown_timeout_behavior,
            },
            _single_threaded: PhantomData,
        }
    }

    pub(crate) fn add_task<F, R>(&self, future: F) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        let future = future.into_future();

        let mut state = self.reentrancy_safe.borrow_mut();

        assert!(
            state.shutdown_deadline.is_none(),
            "Cannot add tasks when the executor is already shutting down"
        );

        // SAFETY: We guarantee that the senders/receivers are dropped before the result channels
        // via a simple technique: we only allow the executor to be dropped when there are no more
        // registered channels because the shutdown process will not complete until all channels
        // have been dropped.
        let (result_tx, result_rx) = unsafe { state.result_events.rent::<R>() };

        let task = state.task_storage.insert(Task::new(future, result_tx));

        let task_ref = TaskRef::new(
            // SAFETY: The task pool itself does not keep any references, so we as the currently
            // only owner of the task have the freedom to create whatever references we desire.
            // In this case we make a reference that lets us access it as a `dyn TypeErasedTask`.
            unsafe { task.cast_type_erased_task() }.into_shared(),
        );

        let wake_signal = WakeSignal::new(
            Arc::clone(&self.shared.awakened),
            Arc::clone(&self.shared.probe_embedded_wake_signals),
            self.shared.owner_waker.clone(),
            task_ref,
        );

        // SAFETY: The task is alive (we own it and just created it) and we are on the thread
        // where it was created (because we just created it). The executor is the only thing that
        // creates references to the tasks and it only ever creates temporary non-overlapping
        // references narrowly bounded to individual code blocks, ensuring that aliasing rules
        // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
        // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
        let task = unsafe { task_ref.as_task() };

        // SAFETY: We are required not to drop the task until it is inert. That is enforced by the
        // shutdown logic - the executor will refuse to shut down as long as any task remains that
        // is not yet in an inert state. We are also required to call this at most once - we do.
        unsafe { task.initialize(wake_signal) };

        state.new_tasks.push_back(task_ref);

        JoinHandle::new(result_rx)
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
            let mut state_reentrant = self.reentrancy_safe.borrow_mut();

            self.drop_inert_tasks(&mut state_exclusive, &mut state_reentrant);

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

        // Accepting a task just means moving it to the active list. We cannot do this immediately
        // when a task is queued because the active task list may be locked by `execute_cycle()`,
        // as new tasks may be enqueued even during an active processing cycle.
        state_exclusive.active.append(&mut state_reentrant.new_tasks);
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
            let mut awakened = self.shared.awakened.lock().expect(ERR_POISONED_LOCK);

            awakened.drain(..).for_each(|task_ref| {
                // It is theoretically possible for a completed task to be awakened, in which case
                // we do nothing. We detect this by ensuring that the task was in the "inactive" set
                // before we react to the wake notification. This also eliminates spurious wakes.
                if state_exclusive.inactive.remove(&task_ref) {
                    state_exclusive.active.push_back(task_ref);

                    TASKS_ACTIVATED_VIA_AWAKENED_SET.with(Event::observe_once);
                } else {
                    TASKS_ACTIVATED_SPURIOUS.with(Event::observe_once);
                }
            });
        }

        // If we have been instructed to probe the embedded wake signals, we do so now.
        // This generally means the `awakened` queue was either full or locked, so could not
        // be used. It switches us into a less efficient mode here but what can you do - if
        // a lot of things are happening, someone has to pay the price.
        //
        // We use Acquire ordering as we are acquiring the synchronization block for the
        // wake-up flags inside the task wake signals.
        if !self.shared.probe_embedded_wake_signals.swap(false, atomic::Ordering::Acquire) {
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
                    // The task has completed, so we can move it to the completed list.
                    // It will sit there until it signals `is_inert()` at which point it is dropped.
                    // It may sit in the `completed` list essentially forever, for example if
                    // something is still holding its waker. We generally hope this is not the
                    // case, though, since that would be wasteful, but we allow it technically.
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
    fn drop_inert_tasks(&self, state_exclusive: &mut ExclusiveState, state_reentrant: &mut ReentrancySafeState) {
        let completed_before = state_exclusive.completed.len();

        // We drop all completed tasks that are inert, which means they have
        // 1) been polled to completion (or aborted); 2) no remaining demands on their resources.
        state_exclusive.completed.retain(|task_ref| {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            if task.is_inert() {
                // SAFETY: The task is still alive (we own it) and we are accessing it from the
                // same thread as it was created on (the executor is single-threaded). All is well.
                let pool_ticket = unsafe { task_ref.into_pool_ticket() };

                // SAFETY: This is the only time we are removing this task because that only happens
                // when a task is removed from the "completed" set, which can only happen once.
                unsafe {
                    state_reentrant.task_storage.remove(pool_ticket);
                }

                false
            } else {
                true
            }
        });

        let dropped_count = completed_before
            .checked_sub(state_exclusive.completed.len())
            .expect("collection cannot grow during item removal");

        if dropped_count > 0 {
            TASKS_DROPPED.with(|x| x.observe(dropped_count));
        }
    }

    /// Whether an immediately performed `execute_cycle()` would do any useful work.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // It timeouts
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
            || !self.shared.awakened.lock().expect(ERR_POISONED_LOCK).is_empty()
            || self.shared.probe_embedded_wake_signals.load(atomic::Ordering::Relaxed)
    }

    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    pub(crate) fn begin_shutdown(&self) {
        let mut state_exclusive = self.exclusive.borrow_mut();
        let mut state_reentrant = self.reentrancy_safe.borrow_mut();

        assert!(
            state_reentrant.shutdown_deadline.is_none(),
            "Cannot start shutdown process when it has already been started"
        );

        let shutdown_start_time = state_exclusive.clock.instant();
        state_reentrant.shutdown_deadline = Some(
            shutdown_start_time
                .checked_add(self.shared.shutdown_timeout)
                .expect("shutdown timeout must be representable as an Instant after shutdown starts"),
        );

        // We call `abort()` on all tasks that we are canceling. This will drop the maximum amount
        // of internal state such as any captured variables that may be holding on to join handles
        // and/or wakers, making it possible to start dropping the tasks. Not all tasks become inert
        // because of this - there may also be callers on other threads holding on to our wakers, in
        // which case the shutdown process will take longer (up to infinity/timeout, e.g. if some
        // external thread is holding on to a waker forever).

        // Needed for split borrowing.
        let state_exclusive_real: &mut ExclusiveState = &mut state_exclusive;

        let new = &mut state_reentrant.new_tasks;
        let active = &mut state_exclusive_real.active;
        let inactive = &mut state_exclusive_real.inactive;
        let completed = &mut state_exclusive_real.completed;

        TASKS_ABORTED_ON_SHUTDOWN.with(|x| {
            x.batch(new.len().saturating_add(active.len().saturating_add(inactive.len())))
                .observe_once();
        });

        for task_ref in active.drain(..).chain(inactive.drain()).chain(new.drain(..)) {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            task.abort();

            completed.push_back(task_ref);
        }
    }

    /// Returns whether we are at the end of a successful shutdown process.
    #[cfg_attr(test, mutants::skip)] // Mutation can lead to deadlocked executor as it never shuts down.
    #[must_use]
    fn evaluate_shutdown_completion(&self, state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) -> bool {
        let Some(shutdown_deadline) = state_reentrant.shutdown_deadline else {
            // We are not in a shutdown process.
            return false;
        };

        // During shutdown, tasks can only exist in the "completed" state.
        debug_assert!(state_exclusive.active.is_empty());
        debug_assert!(state_reentrant.new_tasks.is_empty());
        debug_assert!(state_exclusive.inactive.is_empty());

        // Shutdown is finished if all of the following are true:
        // 1. All completed tasks (== all tasks because there can be
        //   no tasks in other states during shutdown) have been removed from the
        //   completed list after they became inert.
        // 2. All registered result channels have been dropped.
        if state_exclusive.completed.is_empty() && state_reentrant.result_events.is_empty() {
            return true;
        }

        if state_exclusive.clock.instant() >= shutdown_deadline {
            self.shutdown_failed(state_exclusive, state_reentrant);
        }

        // Shutdown process is ongoing.
        false
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // The default behavior terminates the test process.
    fn shutdown_failed(&self, state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) {
        #[cfg(debug_assertions)]
        self.report_shutdown_diagnostics(state_exclusive, state_reentrant);

        // We write to standard error stream because the logging system is going to stop
        // functioning shortly, so any data written to logs might not survive.
        eprintln!(
            "Executor shutdown timed out with {} tasks and {} JoinHandles having not completed cleanup. Use a debug build with RUST_BACKTRACE=1 to emit maximum diagnostic information.",
            state_exclusive.completed.len(),
            state_reentrant.result_events.len()
        );

        match self.shared.shutdown_timeout_behavior {
            ShutdownTimeoutBehavior::TerminateProcess => std::process::abort(),
            #[cfg(test)]
            ShutdownTimeoutBehavior::Panic => panic!("executor shutdown timed out"),
        }
    }

    #[cfg(debug_assertions)]
    #[cfg_attr(test, mutants::skip)] // Purely telemetry, nothing worth testing.
    fn report_shutdown_diagnostics(&self, state_exclusive: &ExclusiveState, state_reentrant: &ReentrancySafeState) {
        // There are different shutdown-blocking states possible with join handles:
        // 1. The join handle's owner is not awaiting it, they just put it in their pocket and
        //    never used it.
        // 2. The join handle's owner started awaiting it and has been woken up but has not
        //    picked up the result for some reason.
        //
        // In all cases, every result channel used by a join handle will have its result set
        // at shutdown time because we disconnect every result channel when shutdown starts. That
        // is a type of result and can be received by whoever owns the join handle, if they are
        // actively awaiting the result, leading to successful cleanup.
        //
        // Join handles owned by other tasks in the same executor will never show up here because
        // we drop all state of all tasks at shutdown time. Unless the task smuggled the join handle
        // out into some non-local state, all such join handles will be dropped by now.
        //
        // In other words, it is OK to not await the join handles, and it is OK to stash it in some
        // independent storage but not both at the same time - that will lead to shutdown timeout.

        use std::num::NonZero;
        state_reentrant.result_events.inspect_awaiters(report_blocking_join_handle);

        // The above covers join handles that are awaited. We can use the total count to identify
        // whether there are also some that are not being awaited at all. We do not generally expect
        // a giant data set from here, so it should be obvious enough.
        if let Some(blocking_join_handle_count) = NonZero::new(state_reentrant.result_events.len()) {
            eprintln!("{blocking_join_handle_count} total JoinHandles blocking shutdown (awaited or not)");
        }

        // In addition to being blocked by join handles, we can simply be blocked by other resources
        // of tasks being held by external parties. Most commonly this would be the wakers held by
        // the targets of await operations started in these tasks. The general expectation is that
        // when an awaited future is dropped (as is guaranteed by the shutdown process), it also
        // clears all the state associated with that await and drops any registered wakers. However,
        // if the code being awaited is defective or sloppy with its resource management, it may
        // fail to do so.
        //
        // The complexity of the matter here is that a task may await many things over its
        // lifecycle - one task can be the source of many wakers. Therefore, in debug builds we
        // wrap the true waker in a diagnostic waker, remembering the backtrace identifying where
        // it was created. These are what we log here - where was every (remaining) waker cloned.
        state_exclusive.completed.iter().for_each(|task_ref| {
            // SAFETY: The task is alive (we own it and just created it) and we are on the thread
            // where it was created (because we just created it). The executor is the only thing that
            // creates references to the tasks and it only ever creates temporary non-overlapping
            // references narrowly bounded to individual code blocks, ensuring that aliasing rules
            // are upheld. Anything outside `ExecutorCore` only passes `TaskRef` by value, never
            // dereferencing it. Reentrant logic for registering new tasks cannot touch existing tasks.
            let task = unsafe { task_ref.as_task() };

            task.inspect_waker_backtraces(&mut |bt| {
                // We write to standard error stream because the logging system is going to stop
                // functioning shortly, so any data written to logs might not survive.
                eprintln!("Task waker still alive at shutdown. Backtrace of where the waker was created: {bt}");
            });
        });
    }
}

impl Drop for ExecutorCore {
    fn drop(&mut self) {
        if thread::panicking() {
            // We skip the assertions if we are already panicking because a double panic more often
            // does not help anything and may even obscure the initial panic in test runs.
            return;
        }

        assert!(
            self.reentrancy_safe.borrow().shutdown_deadline.is_some(),
            "Executor is being dropped without a shutdown process having been started. This is a programming error."
        );

        let state_exclusive = self.exclusive.get_mut();
        let state_reentrant = self.reentrancy_safe.get_mut();
        assert!(
            state_exclusive.completed.is_empty() && state_reentrant.result_events.is_empty(),
            "Executor is being dropped before execute_cycle() returned CycleOutcome::Shutdown. This violates ExecutorBuilder::build() safety requirements."
        );
    }
}

#[cfg(debug_assertions)]
#[cfg_attr(test, mutants::skip)] // Purely telemetry, nothing worth testing.
#[cfg_attr(coverage_nightly, coverage(off))]
fn report_blocking_join_handle(bt: &Backtrace) {
    // We write to standard error stream because the logging system is going to stop
    // functioning shortly, so any data written to logs might not survive.
    eprintln!("JoinHandle blocking shutdown. Backtrace of where it was most recently awaited from: {bt}");
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
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[test]
    fn thread_safety() {
        assert_not_impl_any!(ExecutorCore: Send, Sync);
    }
}
