// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{TaskId, WorkerId};
use tick::SimpleClock;

use crate::snapshot::{CaptureOptions, Counters, Runtime, RuntimeState, Task, TaskCollection, TaskMetrics, WorkerState};
use crate::task::TaskRequest;
use crate::{RuntimeControl, WorkerControl, lock};

const MEASUREMENT_TASK_LIMIT: usize = 4096;

/// Serializes overlapping timed users and allocates their shared measurement window.
#[derive(Debug, Default)]
pub(crate) struct Timing {
    next: u64,
    users: usize,
}

/// Bounded per-worker poll aggregates, populated only during a timed capture.
#[derive(Default)]
pub(crate) struct Measurements {
    pub(crate) window: u64,
    pub(crate) tasks: HashMap<TaskId, Measured>,
}

/// Poll totals collected for one task since its first measured poll.
#[derive(Debug)]
pub(crate) struct Measured {
    pub(crate) since: EventTimestamp,
    pub(crate) metrics: TaskMetrics,
}

impl Measurements {
    pub(crate) fn add(&mut self, window: u64, task: TaskId, start: EventTimestamp, elapsed: u64) {
        if self.window != window {
            self.window = window;
            self.tasks.clear();
        }
        if self.tasks.len() == MEASUREMENT_TASK_LIMIT && !self.tasks.contains_key(&task) {
            return;
        }
        let measured = self.tasks.entry(task).or_insert_with(|| Measured {
            since: start,
            metrics: TaskMetrics::default(),
        });
        measured.metrics.poll_count = measured.metrics.poll_count.saturating_add(1);
        measured.metrics.poll_duration_nanos = measured.metrics.poll_duration_nanos.saturating_add(elapsed);
        measured.metrics.max_poll_duration_nanos = measured.metrics.max_poll_duration_nanos.max(elapsed);
    }
}

/// An owned response detached from native task storage.
pub(crate) struct Batch {
    pub(crate) worker: WorkerId,
    pub(crate) tasks: Vec<Task>,
    pub(crate) collection: TaskCollection,
}

/// Holds advisory poll demand; timed users separately own measurement cleanup.
struct Observation<'a> {
    runtime: &'a RuntimeControl,
    timed: bool,
}

impl<'a> Observation<'a> {
    fn new(runtime: &'a RuntimeControl, timed: bool) -> Self {
        // Demand is advisory. Provider channels publish requests; timing and
        // measurements mutexes protect state. A stale demand read can skip a cycle.
        runtime.requests.fetch_add(1, Ordering::Relaxed);
        if timed {
            let mut timing = lock(&runtime.timing);
            if timing.users == 0 {
                timing.next = timing
                    .next
                    .checked_add(1)
                    .expect("a process cannot exhaust u64 observation windows");
            }
            timing.users += 1;
            runtime.timing_window.store(timing.next, Ordering::Release);
        }
        Self { runtime, timed }
    }
}

impl Drop for Observation<'_> {
    fn drop(&mut self) {
        if self.timed {
            let mut timing = lock(&self.runtime.timing);
            timing.users -= 1;
            if timing.users == 0 {
                self.runtime.timing_window.store(0, Ordering::Release);
                for worker in lock(&self.runtime.workers).iter() {
                    lock(&worker.measurements).tasks.clear();
                }
            }
        }
        self.runtime.requests.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn capture(runtime: &Arc<RuntimeControl>, options: CaptureOptions) -> Runtime {
    capture_with(
        runtime,
        options,
        &SimpleClock::new_system(),
        std::thread::sleep,
        mpsc::Receiver::recv_timeout,
    )
}

fn capture_with(
    runtime: &Arc<RuntimeControl>,
    options: CaptureOptions,
    clock: &SimpleClock,
    pause: impl FnOnce(Duration),
    mut receive: impl FnMut(&mpsc::Receiver<Batch>, Duration) -> Result<Batch, mpsc::RecvTimeoutError>,
) -> Runtime {
    let _observation = Observation::new(runtime, !options.measure_for.is_zero());
    if !options.measure_for.is_zero() {
        pause(options.measure_for);
    }
    let window = runtime.timing_window.load(Ordering::Acquire);
    let workers = lock(&runtime.workers).clone();
    let (sender, receiver) = mpsc::sync_channel(workers.len().max(1));
    let canceled = Arc::new(AtomicBool::new(false));
    let deadline = clock.instant() + options.timeout;
    let mut pending = HashSet::new();
    let mut batches = HashMap::new();
    for worker in &workers {
        if let Some(batch) = dispatch(worker, &sender, &canceled, options.limit_per_worker, window) {
            batches.insert(worker.id, batch);
        } else {
            pending.insert(worker.id);
        }
    }
    drop(sender);
    let unavailable = loop {
        if pending.is_empty() {
            break TaskCollection::Complete;
        }
        match receive(&receiver, deadline.saturating_duration_since(clock.instant())) {
            Ok(batch) => {
                if pending.remove(&batch.worker) {
                    batches.insert(batch.worker, batch);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break TaskCollection::TimedOut,
            Err(mpsc::RecvTimeoutError::Disconnected) => break TaskCollection::Failed,
        }
    };
    canceled.store(true, Ordering::Release);
    for id in pending {
        batches.insert(
            id,
            Batch {
                worker: id,
                tasks: Vec::new(),
                collection: unavailable,
            },
        );
    }
    let mut tasks = Vec::new();
    let workers = workers
        .iter()
        .map(|worker| {
            let batch = batches
                .remove(&worker.id)
                .expect("every worker has a response or explicit availability");
            tasks.extend(batch.tasks);
            worker.snapshot(batch.collection)
        })
        .collect();
    tasks.sort_unstable_by_key(|task| task.id);
    let state = RuntimeState::from_wire_value(runtime.state.load(Ordering::Acquire)).expect("runtime state stores enum values");
    Runtime {
        id: runtime.id,
        name: runtime.name.clone(),
        configured_workers: runtime.configured_workers,
        lifecycle_backtraces: runtime.lifecycle_backtraces,
        state,
        created_at: runtime.created_at,
        retired_at: (state == RuntimeState::Stopped).then(|| EventTimestamp::from_ticks(runtime.retired_at.load(Ordering::Acquire))),
        counters: Counters::default(),
        counters_scope: crate::snapshot::CountersScope::Uncollected,
        workers,
        tasks,
    }
}

fn dispatch(
    worker: &Arc<WorkerControl>,
    sender: &mpsc::SyncSender<Batch>,
    canceled: &Arc<AtomicBool>,
    limit: usize,
    window: u64,
) -> Option<Batch> {
    let provider = lock(&worker.provider).clone();
    // AcqRel claims the slot after the previous request's Release in Drop.
    // The channel, not this flag, publishes the owned request and its response.
    let unavailable = if worker.state.load(Ordering::Acquire) == WorkerState::Stopped.wire_value() {
        Some(TaskCollection::Stopped)
    } else if provider.is_none() {
        Some(TaskCollection::Unavailable)
    } else if worker.request_pending.swap(true, Ordering::AcqRel) {
        Some(TaskCollection::Busy)
    } else {
        None
    };
    if let Some(collection) = unavailable {
        return Some(Batch {
            worker: worker.id,
            tasks: Vec::new(),
            collection,
        });
    }
    let request = TaskRequest {
        worker: Arc::clone(worker),
        sender: sender.clone(),
        canceled: Arc::clone(canceled),
        limit,
        window,
        answered: false,
    };
    let provider = provider.expect("only workers with a provider reach request dispatch");
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| provider(request))) {
        discard_panic(panic);
        return Some(Batch {
            worker: worker.id,
            tasks: Vec::new(),
            collection: TaskCollection::Failed,
        });
    }
    None
}

// A diagnostic callback can panic with a payload whose destructor also panics.
// Bound replacement disposal just as the executor does; never leak user unwinds.
fn discard_panic(panic: Box<dyn std::any::Any + Send>) {
    discard_panic_with(panic, |panic| abort_repeated_panic(panic));
}

fn discard_panic_with(mut panic: Box<dyn std::any::Any + Send>, terminal: impl FnOnce(Box<dyn std::any::Any + Send>)) {
    const MAX_DISPOSAL_ATTEMPTS: usize = 32;
    for _ in 0..MAX_DISPOSAL_ATTEMPTS {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(panic))) {
            Ok(()) => return,
            Err(replacement) => panic = replacement,
        }
    }
    terminal(panic);
}

#[cfg_attr(coverage_nightly, coverage(off))] // Terminal repeated-panic disposal aborts the process.
#[cfg_attr(test, mutants::skip)] // Intentional terminal fallback, not recoverable diagnostic behavior.
fn abort_repeated_panic(_panic: Box<dyn std::any::Any + Send>) -> ! {
    std::process::abort()
}

impl std::fmt::Debug for Measurements {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("window", &self.window)
            .field("tasks", &self.tasks.len())
            .finish()
    }
}

impl std::fmt::Debug for Batch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("worker", &self.worker)
            .field("collection", &self.collection)
            .field("tasks", &self.tasks.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{TaskActivityState, TaskMetricsScope};
    use crate::worker::{WorkerMetadata, WorkerRole};
    use crate::{RuntimeMetadata, allocate_task_id, allocate_type_descriptor_id, register_runtime};

    fn ready(receiver: &mpsc::Receiver<Batch>, _remaining: Duration) -> Result<Batch, mpsc::RecvTimeoutError> {
        receiver.try_recv().map_err(|error| match error {
            mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
            mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
        })
    }

    fn capture_state(runtime: &Arc<RuntimeControl>) -> Runtime {
        capture_with(
            runtime,
            CaptureOptions::default(),
            &SimpleClock::new_frozen(),
            |_| panic!("untimed capture"),
            ready,
        )
    }

    #[test]
    fn a_poll_from_a_finished_window_cannot_enter_the_next_window() {
        let runtime = register_runtime(RuntimeMetadata::new("measurement-window", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let id = allocate_task_id();
        let descriptor = allocate_type_descriptor_id();
        handle
            .register_task_provider(move |request| {
                request.respond(
                    vec![Task::new(id, descriptor, TaskActivityState::Waiting)],
                    TaskCollection::Complete,
                );
            })
            .unwrap();
        let first = Observation::new(&runtime.handle.control, true);
        let stale_poll = handle.poll_started_with(id, || EventTimestamp::from_ticks(10));
        drop(first);
        let second = Observation::new(&runtime.handle.control, true);
        handle.poll_finished_with(stale_poll, || EventTimestamp::from_ticks(30));
        let stale = capture_state(&runtime.handle.control);

        let poll = handle.poll_started_with(id, || EventTimestamp::from_ticks(40));
        handle.poll_finished_with(poll, || EventTimestamp::from_ticks(97));
        let current = capture_state(&runtime.handle.control);
        drop(second);

        assert_eq!(
            (stale.tasks[0].metrics_scope, current.tasks[0].metrics, handle.is_observed()),
            (
                TaskMetricsScope::Uncollected,
                TaskMetrics {
                    poll_count: 1,
                    poll_duration_nanos: 57,
                    max_poll_duration_nanos: 57,
                    ..TaskMetrics::default()
                },
                false
            )
        );
    }

    #[test]
    fn concurrent_requests_report_busy_until_the_owner_explicitly_answers() {
        enum Progress {
            Request(TaskRequest),
            Finished,
        }
        struct Finished(mpsc::Sender<Progress>);
        impl Drop for Finished {
            fn drop(&mut self) {
                // The test can fail before the caller exits.
                let _closed = self.0.send(Progress::Finished);
            }
        }
        let runtime = register_runtime(RuntimeMetadata::new("controlled-concurrent", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let (sender, receiver) = mpsc::channel();
        handle
            .register_task_provider({
                let sender = sender.clone();
                move |request| sender.send(Progress::Request(request)).unwrap()
            })
            .unwrap();
        let caller = runtime.handle();
        let capture = std::thread::spawn(move || {
            let _finished = Finished(sender);
            capture_with(
                &caller.control,
                CaptureOptions::default(),
                &SimpleClock::new_frozen(),
                |_| panic!("untimed capture"),
                |receiver, _| receiver.recv().map_err(|_closed| mpsc::RecvTimeoutError::Disconnected),
            )
        });
        let Progress::Request(request) = receiver.recv().unwrap() else {
            panic!("capture finished without dispatching a request");
        };
        assert!(handle.is_observed());

        let busy = capture_state(&runtime.handle.control);
        request.respond(Vec::new(), TaskCollection::Complete);
        let answered = capture.join().unwrap();

        assert_eq!(
            (
                busy.workers[0].task_collection,
                answered.workers[0].task_collection,
                handle.is_observed()
            ),
            (TaskCollection::Busy, TaskCollection::Complete, false)
        );
    }

    #[test]
    fn dropped_request_is_failed_even_while_another_worker_times_out() {
        let runtime = register_runtime(RuntimeMetadata::new("per-worker-failure", 2));
        let dropped = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let retained = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        dropped.handle().register_task_provider(drop).unwrap();
        let (sender, receiver) = mpsc::channel();
        retained
            .handle()
            .register_task_provider(move |request| sender.send(request).unwrap())
            .unwrap();

        let result = capture_state(&runtime.handle.control);
        let request = receiver.try_recv().unwrap();

        assert_eq!(
            (
                result.workers[0].task_collection,
                result.workers[1].task_collection,
                request.is_canceled(),
                request.limit()
            ),
            (TaskCollection::Failed, TaskCollection::TimedOut, true, 256)
        );
        drop(request);
        assert!(!retained.handle().is_observed());
    }

    #[test]
    fn disconnected_collection_cancels_late_answers_and_releases_the_next_request() {
        let runtime = register_runtime(RuntimeMetadata::new("disconnected-collection", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let (sender, receiver) = mpsc::channel();
        handle.register_task_provider(move |request| sender.send(request).unwrap()).unwrap();

        let failed = capture_with(
            &runtime.handle.control,
            CaptureOptions::default(),
            &SimpleClock::new_frozen(),
            |_| panic!("untimed capture"),
            |_, _| Err(mpsc::RecvTimeoutError::Disconnected),
        );
        let request = receiver.try_recv().unwrap();
        let canceled = request.is_canceled();
        let debug_is_empty = format!("{request:?}").is_empty();
        request.respond(Vec::new(), TaskCollection::Complete);
        handle
            .register_task_provider(|request| request.respond(Vec::new(), TaskCollection::Complete))
            .unwrap();
        let retried = capture_state(&runtime.handle.control);

        assert_eq!(
            (
                failed.workers[0].task_collection,
                canceled,
                debug_is_empty,
                retried.workers[0].task_collection,
                handle.is_observed()
            ),
            (TaskCollection::Failed, true, false, TaskCollection::Complete, false)
        );
    }

    #[test]
    fn response_receiver_gets_the_configured_deadline_budget() {
        let runtime = register_runtime(RuntimeMetadata::new("response-deadline", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let (sender, receiver) = mpsc::channel();
        worker
            .handle()
            .register_task_provider(move |request| sender.send(request).unwrap())
            .unwrap();
        let timeout = Duration::from_millis(42);
        let mut budget = None;

        let captured = capture_with(
            &runtime.handle.control,
            CaptureOptions {
                timeout,
                ..CaptureOptions::default()
            },
            &SimpleClock::new_frozen(),
            |_| panic!("untimed capture"),
            |_, remaining| {
                budget = Some(remaining);
                Err(mpsc::RecvTimeoutError::Timeout)
            },
        );
        let canceled = receiver.try_recv().unwrap().is_canceled();

        assert_eq!(
            (budget, captured.workers[0].task_collection, canceled),
            (Some(timeout), TaskCollection::TimedOut, true)
        );
    }

    #[test]
    fn response_transport_tolerates_disconnection_but_rejects_a_broken_capacity_invariant() {
        let runtime = register_runtime(RuntimeMetadata::new("response-transport", 1));
        let _worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let worker = Arc::clone(&lock(&runtime.handle.control.workers)[0]);
        for connected in [false, true] {
            let (sender, receiver) = mpsc::sync_channel(0);
            let _receiver = connected.then_some(receiver);
            worker.request_pending.store(true, Ordering::Relaxed);
            let request = TaskRequest {
                worker: Arc::clone(&worker),
                sender,
                canceled: Arc::new(AtomicBool::new(false)),
                limit: 1,
                window: 0,
                answered: false,
            };

            let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                request.respond(Vec::new(), TaskCollection::Complete);
            }));

            assert_eq!(
                (response.is_err(), worker.request_pending.load(Ordering::Acquire)),
                (connected, false)
            );
        }
    }

    #[test]
    fn a_provider_panic_overrides_a_response_sent_before_the_panic() {
        let runtime = register_runtime(RuntimeMetadata::new("response-before-panic", 2));
        let failed = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let complete = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        failed
            .handle()
            .register_task_provider(|request| {
                request.respond(Vec::new(), TaskCollection::Complete);
                panic!("provider failed after answering");
            })
            .unwrap();
        complete
            .handle()
            .register_task_provider(|request| request.respond(Vec::new(), TaskCollection::Complete))
            .unwrap();

        let captured = capture_state(&runtime.handle.control);

        assert_eq!(
            captured.workers.iter().map(|worker| worker.task_collection).collect::<Vec<_>>(),
            [TaskCollection::Failed, TaskCollection::Complete]
        );
    }

    #[test]
    fn provider_panic_payload_is_disposed_without_losing_failure_status() {
        struct PanicOnce(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for PanicOnce {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                panic!("payload disposal");
            }
        }
        let runtime = register_runtime(RuntimeMetadata::new("provider-panic", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        worker
            .handle()
            .register_task_provider({
                let drops = Arc::clone(&drops);
                move |_request| std::panic::panic_any(PanicOnce(Arc::clone(&drops)))
            })
            .unwrap();

        let result = capture_state(&runtime.handle.control);

        assert_eq!(
            (result.workers[0].task_collection, drops.load(Ordering::Relaxed)),
            (TaskCollection::Failed, 1)
        );
    }

    #[test]
    fn endlessly_replaced_panic_payloads_reach_the_terminal_boundary() {
        struct Repeated {
            drops: Arc<std::sync::atomic::AtomicUsize>,
            stopped: Arc<AtomicBool>,
        }
        impl Drop for Repeated {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::Relaxed);
                if !self.stopped.load(Ordering::Relaxed) {
                    std::panic::panic_any(Self {
                        drops: Arc::clone(&self.drops),
                        stopped: Arc::clone(&self.stopped),
                    });
                }
            }
        }
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let panic = Box::new(Repeated {
            drops: Arc::clone(&drops),
            stopped: Arc::clone(&stopped),
        });
        let mut terminal_at = None;

        let disposed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            discard_panic_with(panic, |panic| {
                stopped.store(true, Ordering::Relaxed);
                terminal_at = Some(drops.load(Ordering::Relaxed));
                drop(panic);
            });
        }));
        let completed = disposed.is_ok();
        // A broken disposer must fail the assertion, not restart the replacement
        // chain while the test harness disposes its escaping panic.
        stopped.store(true, Ordering::Relaxed);
        drop(disposed);

        assert_eq!((completed, terminal_at, drops.load(Ordering::Relaxed)), (true, Some(32), 33));
    }

    #[test]
    fn overlapping_timed_observers_keep_measurements_until_the_last_user_leaves() {
        let runtime = register_runtime(RuntimeMetadata::new("overlapping-windows", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let id = allocate_task_id();
        let descriptor = allocate_type_descriptor_id();
        worker
            .handle()
            .register_task_provider(move |request| {
                request.respond(
                    vec![Task::new(id, descriptor, TaskActivityState::Waiting)],
                    TaskCollection::Complete,
                );
            })
            .unwrap();
        let first = Observation::new(&runtime.handle.control, true);
        let second = Observation::new(&runtime.handle.control, true);
        let window = runtime.handle.control.timing_window.load(Ordering::Acquire);
        let handle = worker.handle();
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let poll = handle.poll_started_with(id, || EventTimestamp::from_ticks(10));
                    handle.poll_finished_with(poll, || EventTimestamp::from_ticks(50));
                })
                .join()
                .unwrap();
            drop(first);
            let retained = capture_state(&runtime.handle.control);
            assert_eq!(
                (
                    runtime.handle.control.timing_window.load(Ordering::Acquire),
                    retained.tasks[0].metrics.poll_count
                ),
                (window, 1)
            );
            drop(second);
        });

        let workers = lock(&runtime.handle.control.workers);
        assert_eq!((handle.is_observed(), lock(&workers[0].measurements).tasks.len()), (false, 0));
    }

    #[test]
    fn timed_capture_waits_for_the_requested_interval_and_releases_demand() {
        let runtime = register_runtime(RuntimeMetadata::new("capture-pause", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let task = allocate_task_id();
        let descriptor = allocate_type_descriptor_id();
        handle
            .register_task_provider(move |request| {
                request.respond(
                    vec![Task::new(task, descriptor, TaskActivityState::Waiting)],
                    TaskCollection::Complete,
                );
            })
            .unwrap();
        let measure_for = Duration::from_secs(1);
        let mut waited = None;

        let result = capture_with(
            &runtime.handle.control,
            CaptureOptions {
                measure_for,
                ..CaptureOptions::default()
            },
            &SimpleClock::new_frozen(),
            |duration| {
                waited = Some(duration);
                let poll = handle.poll_started_with(task, || EventTimestamp::from_ticks(10));
                handle.poll_finished_with(poll, || EventTimestamp::from_ticks(50));
            },
            ready,
        );

        assert_eq!(
            (
                waited,
                result.workers[0].task_collection,
                result.tasks[0].metrics_scope,
                result.tasks[0].metrics,
                handle.is_observed()
            ),
            (
                Some(measure_for),
                TaskCollection::Complete,
                TaskMetricsScope::PollsSince(EventTimestamp::from_ticks(10)),
                TaskMetrics {
                    poll_count: 1,
                    poll_duration_nanos: 40,
                    max_poll_duration_nanos: 40,
                    ..TaskMetrics::default()
                },
                false
            )
        );
    }

    #[test]
    fn state_only_capture_does_not_read_poll_clocks_or_open_measurement_windows() {
        let runtime = register_runtime(RuntimeMetadata::new("untimed-polls", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let task = allocate_task_id();
        let descriptor = allocate_type_descriptor_id();
        handle
            .register_task_provider({
                let handle = handle.clone();
                move |request| {
                    let poll = handle.poll_started_with(task, || panic!("state-only capture read a start clock"));
                    handle.poll_finished_with(poll, || panic!("state-only capture read a finish clock"));
                    request.respond(
                        vec![Task::new(task, descriptor, TaskActivityState::Waiting)],
                        TaskCollection::Complete,
                    );
                }
            })
            .unwrap();

        let result = capture_state(&runtime.handle.control);

        assert_eq!(
            (
                result.workers[0].task_collection,
                result.tasks[0].metrics_scope,
                result.tasks[0].metrics
            ),
            (TaskCollection::Complete, TaskMetricsScope::Uncollected, TaskMetrics::default())
        );
    }

    #[test]
    fn recording_only_poll_tokens_do_not_populate_measurement_storage() {
        let runtime = register_runtime(RuntimeMetadata::new("recording-without-measurement", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let mut poll = handle.poll_started_with(allocate_task_id(), || EventTimestamp::from_ticks(10));
        // A recording-only token has a timestamp but no measurement window.
        poll.started_at = Some(EventTimestamp::from_ticks(10));

        handle.poll_finished_with(poll, || EventTimestamp::from_ticks(50));
        let workers = lock(&runtime.handle.control.workers);

        assert_eq!(lock(&workers[0].measurements).tasks.len(), 0);
    }

    #[test]
    fn measurement_capacity_retains_existing_tasks_and_omits_new_unmeasured_tasks() {
        let mut measured = Measurements::default();
        let first = allocate_task_id();
        measured.add(1, first, EventTimestamp::from_ticks(10), 20);
        for _ in 1..MEASUREMENT_TASK_LIMIT {
            measured.add(1, allocate_task_id(), EventTimestamp::from_ticks(10), 2);
        }
        measured.add(1, first, EventTimestamp::from_ticks(40), 30);
        let omitted = allocate_task_id();
        measured.add(1, omitted, EventTimestamp::from_ticks(40), 1);

        assert_eq!(
            (
                measured.tasks.len(),
                measured.tasks[&first].metrics,
                measured.tasks.contains_key(&omitted)
            ),
            (
                MEASUREMENT_TASK_LIMIT,
                TaskMetrics {
                    poll_count: 2,
                    poll_duration_nanos: 50,
                    max_poll_duration_nanos: 30,
                    ..TaskMetrics::default()
                },
                false
            )
        );
        assert!(!format!("{measured:?}").is_empty());
        assert!(
            !format!(
                "{:?}",
                Batch {
                    worker: WorkerId::from_raw(1).unwrap(),
                    tasks: Vec::new(),
                    collection: TaskCollection::Complete
                }
            )
            .is_empty()
        );
    }
}
