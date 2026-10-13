// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded requests for runtime-owned task data, without a mirrored task registry.
//!
//! A provider receives a [`TaskRequest`] on the requesting thread and routes it to
//! its worker. The worker inspects its native task storage at a safe point and
//! responds with owned descriptions. User callbacks run on the subscription's
//! diagnostic thread, never on a runtime worker. Diagnostic setup, collection,
//! and callback delivery are suppressed from event recording.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};
use std::{fmt, thread};

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{RuntimeId, WorkerId};

use crate::snapshot::{Task, WorkerState};
use crate::{RuntimeControl, RuntimeHandle, WorkerControl, lock, registry};

const MAX_TASKS_PER_WORKER: usize = 4096;
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(50);

/// Failure to configure or start live task delivery.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
}

#[derive(Debug)]
enum ErrorKind {
    InvalidOptions(&'static str),
    RuntimeUnavailable(RuntimeId),
    ThreadSpawn(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::InvalidOptions(reason) => f.write_str(reason),
            ErrorKind::RuntimeUnavailable(id) => write!(f, "runtime {} is not registered", id.get()),
            ErrorKind::ThreadSpawn(source) => write!(f, "failed to start live task delivery: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::ThreadSpawn(source) => Some(source),
            ErrorKind::InvalidOptions(_) | ErrorKind::RuntimeUnavailable(_) => None,
        }
    }
}

/// Outcome of inspecting one worker's live task storage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum TaskCollection {
    /// This worker uses retained telemetry rather than a live provider.
    #[default]
    Retained = 0,
    /// Every live task was inspected.
    Complete = 1,
    /// The task limit was reached.
    Truncated = 2,
    /// A previous request or execution still occupies the worker.
    Busy = 3,
    /// The worker did not respond before the request deadline.
    TimedOut = 4,
    /// The worker has stopped and owns no executable tasks.
    Stopped = 5,
    /// The provider failed to deliver a response.
    Failed = 6,
}

impl TaskCollection {
    pub(crate) const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Retained),
            1 => Some(Self::Complete),
            2 => Some(Self::Truncated),
            3 => Some(Self::Busy),
            4 => Some(Self::TimedOut),
            5 => Some(Self::Stopped),
            6 => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Limits for one task-data request.
#[derive(Clone, Copy, Debug)]
pub struct QueryOptions {
    /// Maximum native tasks inspected per worker, including tasks without descriptions.
    pub limit_per_worker: NonZeroUsize,
    /// Total time allowed for all workers to respond.
    pub timeout: Duration,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            limit_per_worker: NonZeroUsize::new(256).expect("the default task limit is nonzero"),
            timeout: Duration::from_millis(50),
        }
    }
}

impl QueryOptions {
    fn validate(self) -> Result<(), Error> {
        if self.limit_per_worker.get() > MAX_TASKS_PER_WORKER || self.timeout.is_zero() || self.timeout > Duration::from_secs(5) {
            return Err(Error {
                kind: ErrorKind::InvalidOptions(
                    "live task queries require 1..=4096 tasks per worker and a timeout greater than zero and at most five seconds",
                ),
            });
        }
        Ok(())
    }
}

/// Owned results from one worker; missing results are never represented as an empty complete list.
#[derive(Clone, Debug)]
pub struct TaskBatch {
    /// Worker whose native storage was inspected.
    pub worker_id: WorkerId,
    /// Time the batch was produced, not a process-wide atomic snapshot boundary.
    pub observed_at: EventTimestamp,
    /// Descriptions of tasks still alive when this worker responded.
    pub tasks: Vec<Task>,
    /// Completeness or explicit reason that task data is unavailable.
    pub collection: TaskCollection,
}

/// One pending request, moved to its worker and answered at most once.
#[derive(Debug)]
pub struct TaskRequest {
    worker: Arc<WorkerControl>,
    sender: mpsc::SyncSender<TaskBatch>,
    canceled: Arc<AtomicBool>,
    limit: NonZeroUsize,
}

impl TaskRequest {
    /// Maximum native tasks to inspect for this response.
    #[must_use]
    pub const fn limit(&self) -> NonZeroUsize {
        self.limit
    }

    /// Whether the requester has stopped waiting for this response.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    /// Delivers owned results without blocking the worker.
    ///
    /// Results after cancellation are intentionally discarded. Excess descriptions
    /// are truncated and reported as incomplete even if the provider claimed completeness.
    ///
    /// # Panics
    ///
    /// Panics if an internal query-channel invariant is violated.
    pub fn respond(self, mut tasks: Vec<Task>, mut collection: TaskCollection) {
        if self.is_canceled() {
            return;
        }
        if tasks.len() > self.limit.get() {
            tasks.truncate(self.limit.get());
            collection = TaskCollection::Truncated;
        }
        let response = TaskBatch {
            worker_id: self.worker.id,
            observed_at: EventTimestamp::now(),
            tasks,
            collection,
        };
        // One response per worker fits the channel capacity. A disconnected
        // receiver means the deadline or subscription cancellation won the race.
        match self.sender.try_send(response) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                unreachable!("one response per worker must fit the task-query channel");
            }
        }
    }
}

impl Drop for TaskRequest {
    fn drop(&mut self) {
        self.worker.request_pending.store(false, Ordering::Release);
    }
}

pub(crate) struct TaskProvider(pub(crate) Arc<dyn Fn(TaskRequest) + Send + Sync>);

impl Clone for TaskProvider {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl fmt::Debug for TaskProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TaskProvider")
    }
}

/// Measurements requested for the duration of a live subscription.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Measurements {
    /// Inspect scheduling state without reading clocks on task polls.
    #[default]
    State,
    /// Also measure task polls while at least one subscriber requests them.
    PollTiming,
}

/// Configuration for a repeating callback subscription.
#[derive(Clone, Copy, Debug)]
pub struct SubscriptionOptions {
    /// Bounded request limits shared by each refresh.
    pub query: QueryOptions,
    /// Delay between refreshes; must be between 50 milliseconds and one minute.
    pub refresh_interval: Duration,
    /// Additional measurements to collect while this subscription is alive.
    pub measurements: Measurements,
}

impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            query: QueryOptions::default(),
            refresh_interval: Duration::from_millis(250),
            measurements: Measurements::State,
        }
    }
}

#[derive(Debug, Default)]
struct Cancellation {
    stopped: AtomicBool,
    wait: Mutex<()>,
    ready: Condvar,
    error: Mutex<Option<String>>,
    timing: Mutex<Option<TimingLease>>,
    observation: Mutex<Option<ObservationLease>>,
}

/// Cancels live delivery and releases requested measurements when dropped.
///
/// Drop does not wait for a user callback to return. A slow callback can delay
/// its own diagnostic thread, but cannot block a worker or retain measurement demand.
#[derive(Debug)]
pub struct Subscription {
    cancellation: Arc<Cancellation>,
}

impl Subscription {
    /// Returns a callback or provider failure reported by this diagnostic thread.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        lock(&self.cancellation.error).clone()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _wait = lock(&self.cancellation.wait);
        self.cancellation.stopped.store(true, Ordering::Release);
        self.cancellation.ready.notify_all();
        lock(&self.cancellation.timing).take();
        lock(&self.cancellation.observation).take();
    }
}

#[derive(Debug)]
struct TimingLease {
    runtime: Arc<RuntimeControl>,
}

#[derive(Debug)]
struct ObservationLease {
    runtime: Arc<RuntimeControl>,
}

impl ObservationLease {
    fn new(runtime: Arc<RuntimeControl>) -> Self {
        runtime.live_observers.fetch_add(1, Ordering::Relaxed);
        Self { runtime }
    }
}

impl Drop for ObservationLease {
    fn drop(&mut self) {
        self.runtime.live_observers.fetch_sub(1, Ordering::Relaxed);
    }
}

impl TimingLease {
    fn new(runtime: Arc<RuntimeControl>) -> Self {
        let mut timing = lock(&runtime.live_timing);
        if timing.subscribers == 0 {
            timing.generation = timing
                .generation
                .checked_add(1)
                .expect("live timing generations cannot exhaust u64");
        }
        timing.subscribers += 1;
        runtime.live_timing_generation.store(timing.generation, Ordering::Release);
        drop(timing);
        Self { runtime }
    }
}

impl Drop for TimingLease {
    fn drop(&mut self) {
        let mut timing = lock(&self.runtime.live_timing);
        timing.subscribers -= 1;
        if timing.subscribers == 0 {
            self.runtime.live_timing_generation.store(0, Ordering::Release);
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Timing {
    generation: u64,
    subscribers: usize,
}

/// Repeatedly requests runtime-owned task data and delivers one batch per worker.
///
/// Requests begin immediately. The callback runs serially on a diagnostic thread,
/// without runtime locks. Each worker has at most one pending query, even across
/// subscribers. Dropping the subscription stops new requests; an in-flight callback
/// may finish afterward. No callbacks are invoked on a runtime worker.
///
/// # Errors
///
/// Returns an error for invalid limits, an unknown runtime, or failure to start
/// the diagnostic thread. Callback panics stop delivery and appear in [`Subscription::error`].
pub fn subscribe(
    runtime_id: RuntimeId,
    options: SubscriptionOptions,
    mut callback: impl FnMut(TaskBatch) + Send + 'static,
) -> Result<Subscription, Error> {
    let _suppression = seismograph::recorder::SuppressionGuard::enter();
    options.query.validate()?;
    if options.refresh_interval < MIN_REFRESH_INTERVAL || options.refresh_interval > Duration::from_mins(1) {
        let _suppression = seismograph::recorder::SuppressionGuard::enter();
        return Err(Error {
            kind: ErrorKind::InvalidOptions("live task refresh intervals must be between 50 milliseconds and one minute"),
        });
    }
    let runtime = lock(&registry().runtimes)
        .iter()
        .find(|runtime| runtime.id == runtime_id)
        .cloned()
        .ok_or(Error {
            kind: ErrorKind::RuntimeUnavailable(runtime_id),
        })?;
    let timing = (options.measurements == Measurements::PollTiming).then(|| TimingLease::new(Arc::clone(&runtime)));
    let cancellation = Arc::new(Cancellation::default());
    *lock(&cancellation.timing) = timing;
    *lock(&cancellation.observation) = Some(ObservationLease::new(Arc::clone(&runtime)));
    let stopped = Arc::clone(&cancellation);
    thread::Builder::new()
        .name("seismograph-runtime-live".into())
        .spawn(move || run_subscription(&RuntimeHandle { control: runtime }, &stopped, options, &mut callback))
        .map_err(|source| Error {
            kind: ErrorKind::ThreadSpawn(source),
        })?;
    Ok(Subscription { cancellation })
}

fn run_subscription(handle: &RuntimeHandle, stopped: &Cancellation, options: SubscriptionOptions, callback: &mut impl FnMut(TaskBatch)) {
    let _suppression = seismograph::recorder::SuppressionGuard::enter();
    while !stopped.stopped.load(Ordering::Acquire) {
        for batch in collect(handle, options.query, &stopped.stopped) {
            if stopped.stopped.load(Ordering::Acquire) {
                return;
            }
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(batch))).is_err() {
                *lock(&stopped.error) = Some("the live task callback panicked; delivery stopped".into());
                stopped.stopped.store(true, Ordering::Release);
                lock(&stopped.timing).take();
                lock(&stopped.observation).take();
                return;
            }
        }
        let guard = lock(&stopped.wait);
        let _guard = stopped
            .ready
            .wait_timeout_while(guard, options.refresh_interval, |()| !stopped.stopped.load(Ordering::Acquire))
            .expect("live task cancellation never holds its mutex across user code");
    }
}

pub(crate) fn collect(runtime: &RuntimeHandle, options: QueryOptions, stopped: &AtomicBool) -> Vec<TaskBatch> {
    let _suppression = seismograph::recorder::SuppressionGuard::enter();
    let _observation = ObservationLease::new(Arc::clone(&runtime.control));
    let workers = lock(&runtime.control.workers).clone();
    let (sender, receiver) = mpsc::sync_channel(workers.len().max(1));
    let canceled = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + options.timeout;
    let mut results = Vec::with_capacity(workers.len());
    let mut pending = Vec::new();
    for worker in &workers {
        let provider = lock(&worker.task_provider).clone();
        let collection = if worker.state.load(Ordering::Acquire) == WorkerState::Stopped.wire_value() {
            Some(TaskCollection::Stopped)
        } else if provider.is_none() {
            Some(TaskCollection::Retained)
        } else if worker.request_pending.swap(true, Ordering::AcqRel) {
            Some(TaskCollection::Busy)
        } else {
            None
        };
        if let Some(collection) = collection {
            results.push(empty_batch(worker.id, collection));
            continue;
        }
        let request = TaskRequest {
            worker: Arc::clone(worker),
            sender: sender.clone(),
            canceled: Arc::clone(&canceled),
            limit: options.limit_per_worker,
        };
        let provider = provider.expect("only live providers reach query dispatch");
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (provider.0)(request))).is_err() {
            results.push(empty_batch(worker.id, TaskCollection::Failed));
        } else {
            pending.push(worker.id);
        }
    }
    drop(sender);
    while !pending.is_empty() && !stopped.load(Ordering::Acquire) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(batch) => {
                if let Some(index) = pending.iter().position(|id| *id == batch.worker_id) {
                    pending.swap_remove(index);
                    results.push(batch);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                results.extend(
                    std::mem::take(&mut pending)
                        .into_iter()
                        .map(|id| empty_batch(id, TaskCollection::Failed)),
                );
            }
        }
    }
    canceled.store(true, Ordering::Release);
    results.extend(pending.into_iter().map(|id| empty_batch(id, TaskCollection::TimedOut)));
    results
}

fn empty_batch(worker_id: WorkerId, collection: TaskCollection) -> TaskBatch {
    TaskBatch {
        worker_id,
        observed_at: EventTimestamp::now(),
        tasks: Vec::new(),
        collection,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::worker::{WorkerMetadata, WorkerRole};
    use crate::{RuntimeMetadata, allocate_type_descriptor_id, register_runtime};

    #[test]
    fn inline_tasks_do_not_allocate_registry_controls_or_collect_disabled_poll_timing() {
        let _test = crate::tests::test_lock();
        seismograph::recorder(seismograph::recorder::Configuration::default());
        let runtime = register_runtime(RuntimeMetadata::new("inline", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = runtime.handle();
        let worker_control = Arc::clone(&lock(&handle.control.workers)[0]);
        let mut task = handle.register_live_task(allocate_type_descriptor_id(), None, 0);
        let poll = task.poll_started(&worker.handle());
        assert_eq!(worker_control.current_task.load(Ordering::Acquire), task.id().get());
        task.poll_finished(&worker.handle(), poll);
        task.completed(Some(worker.id()));
        task.completed(Some(worker.id()));
        let counters = handle.counters().snapshot();
        assert_eq!(
            (
                lock(&handle.control.tasks).len(),
                task.snapshot(crate::snapshot::TaskActivityState::Waiting, Some(worker.id()))
                    .metrics_scope,
                counters.completed_tasks,
                counters.live_tasks,
                counters.poll_count,
                worker_control.current_task.load(Ordering::Acquire),
            ),
            (0, crate::snapshot::TaskMetricsScope::Uncollected, 1, 0, 0, 0)
        );
    }

    #[test]
    fn providers_deliver_owned_batches_and_truncate_excessive_results() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("provider", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_live_task(allocate_type_descriptor_id(), None, 7);
        let description = task.snapshot(crate::snapshot::TaskActivityState::Waiting, Some(worker.id()));
        worker.handle().register_task_provider(move |request| {
            request.respond(vec![description.clone(), description.clone()], TaskCollection::Complete);
        });
        let batches = collect(
            &runtime.handle(),
            QueryOptions {
                limit_per_worker: NonZeroUsize::new(1).unwrap(),
                ..QueryOptions::default()
            },
            &AtomicBool::new(false),
        );
        assert_eq!((batches[0].collection, batches[0].tasks.len()), (TaskCollection::Truncated, 1));
    }

    #[test]
    fn an_expired_request_remains_bounded_until_its_worker_discards_it() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("unresponsive", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let pending = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&pending);
        worker.handle().register_task_provider(move |request| {
            *lock(&slot) = Some(request);
        });
        let options = QueryOptions {
            timeout: Duration::from_millis(1),
            ..QueryOptions::default()
        };
        let first = collect(&runtime.handle(), options, &AtomicBool::new(false));
        let second = collect(&runtime.handle(), options, &AtomicBool::new(false));
        let request = lock(&pending).take().unwrap();
        let canceled = request.is_canceled();
        request.respond(Vec::new(), TaskCollection::Complete);
        assert_eq!(
            (
                first[0].collection,
                second[0].collection,
                canceled,
                lock(&runtime.handle.control.workers)[0].request_pending.load(Ordering::Acquire)
            ),
            (TaskCollection::TimedOut, TaskCollection::Busy, true, false)
        );
    }

    #[test]
    fn provider_panics_are_reported_as_failed_collection() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("panic-provider", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        worker.handle().register_task_provider(|_| panic!("provider"));
        let batches = collect(&runtime.handle(), QueryOptions::default(), &AtomicBool::new(false));
        assert_eq!(batches[0].collection, TaskCollection::Failed);
        assert!(!lock(&runtime.handle.control.workers)[0].request_pending.load(Ordering::Acquire));
    }

    #[test]
    fn dropped_requests_and_stopped_workers_have_explicit_outcomes() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("stopped-provider", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        worker.handle().register_task_provider(drop);
        let failed = collect(&runtime.handle(), QueryOptions::default(), &AtomicBool::new(false));
        drop(worker);
        let stopped = collect(&runtime.handle(), QueryOptions::default(), &AtomicBool::new(false));
        assert_eq!(
            (failed[0].collection, stopped[0].collection),
            (TaskCollection::Failed, TaskCollection::Stopped)
        );
    }

    #[test]
    fn timing_demand_is_reference_counted_and_restarts_in_a_new_window() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("timing-requests", 1));
        let first = TimingLease::new(Arc::clone(&runtime.handle.control));
        let generation = runtime.handle.control.live_timing_generation.load(Ordering::Acquire);
        let second = TimingLease::new(Arc::clone(&runtime.handle.control));
        drop(first);
        assert_eq!(runtime.handle.control.live_timing_generation.load(Ordering::Acquire), generation);
        drop(second);
        assert_eq!(runtime.handle.control.live_timing_generation.load(Ordering::Acquire), 0);
        let third = TimingLease::new(Arc::clone(&runtime.handle.control));
        assert!(runtime.handle.control.live_timing_generation.load(Ordering::Acquire) > generation);
        drop(third);
    }

    #[test]
    fn callback_panics_stop_delivery_and_release_timing_demand() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("callback-panic", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        worker
            .handle()
            .register_task_provider(|request| request.respond(Vec::new(), TaskCollection::Complete));
        let subscription = subscribe(
            runtime.id(),
            SubscriptionOptions {
                measurements: Measurements::PollTiming,
                ..SubscriptionOptions::default()
            },
            |_| panic!("callback"),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while subscription.error().is_none() || runtime.handle.control.live_timing_generation.load(Ordering::Acquire) != 0 {
            assert!(
                Instant::now() < deadline,
                "the diagnostic thread reports callback failure and releases its timing lease"
            );
            thread::yield_now();
        }
        assert_eq!(
            subscription.error().as_deref(),
            Some("the live task callback panicked; delivery stopped")
        );
    }

    #[test]
    fn invalid_subscription_limits_and_unknown_runtimes_are_rejected() {
        let _test = crate::tests::test_lock();
        let runtime = register_runtime(RuntimeMetadata::new("invalid-subscription", 0));
        let invalid = [
            SubscriptionOptions {
                refresh_interval: Duration::ZERO,
                ..SubscriptionOptions::default()
            },
            SubscriptionOptions {
                query: QueryOptions {
                    timeout: Duration::ZERO,
                    ..QueryOptions::default()
                },
                ..SubscriptionOptions::default()
            },
            SubscriptionOptions {
                query: QueryOptions {
                    limit_per_worker: NonZeroUsize::new(MAX_TASKS_PER_WORKER + 1).unwrap(),
                    ..QueryOptions::default()
                },
                ..SubscriptionOptions::default()
            },
        ];
        for options in invalid {
            assert!(matches!(
                subscribe(runtime.id(), options, |_| {}).unwrap_err().kind,
                ErrorKind::InvalidOptions(_)
            ));
        }
        assert!(matches!(
            subscribe(RuntimeId::from_raw(u64::MAX).unwrap(), SubscriptionOptions::default(), |_| {})
                .unwrap_err()
                .kind,
            ErrorKind::RuntimeUnavailable(_)
        ));
    }
}
