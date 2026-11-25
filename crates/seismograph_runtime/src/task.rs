// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native-storage requests, task lifecycle events, and observed poll tokens.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TypeDescriptorId, WorkerId};

use crate::snapshot::{Task, TaskCollection, TaskMetrics};
use crate::{WorkerControl, collection, lock};

static TYPE_DESCRIPTORS: OnceLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = OnceLock::new();

fn type_descriptor<T: 'static>() -> TypeDescriptorId {
    let mut descriptors = lock(TYPE_DESCRIPTORS.get_or_init(|| Mutex::new(HashMap::new())));
    *descriptors
        .entry(TypeId::of::<T>())
        .or_insert_with(crate::allocate_type_descriptor_id)
}

/// Creation metadata and identity for one task submission.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TaskProperties {
    id: TaskId,
    descriptor: TypeDescriptorId,
    parent: Option<TaskId>,
    body_size: Option<usize>,
    spawned_at: Option<EventTimestamp>,
    origin: Option<(RuntimeId, WorkerId)>,
}

impl TaskProperties {
    /// Creates immutable task metadata with an explicit identity and descriptor.
    #[must_use]
    pub fn new(id: TaskId, descriptor: TypeDescriptorId) -> Self {
        Self {
            id,
            descriptor,
            parent: None,
            body_size: None,
            spawned_at: None,
            origin: None,
        }
    }

    /// Creates immutable metadata for one concrete task body.
    #[must_use]
    pub(crate) fn for_type<T: 'static>(body_size: usize) -> Self {
        Self::new(crate::allocate_task_id(), type_descriptor::<T>()).body_size(body_size)
    }

    /// Sets the parent task that was active at submission.
    #[must_use]
    pub const fn parent(mut self, parent: Option<TaskId>) -> Self {
        self.parent = parent;
        self
    }

    /// Overrides the body size after wrapper or type erasure.
    #[must_use]
    pub const fn body_size(mut self, body_size: usize) -> Self {
        self.body_size = Some(body_size);
        self
    }

    /// Sets the runtime worker that submitted this task.
    #[must_use]
    pub const fn origin(mut self, runtime: RuntimeId, worker: WorkerId) -> Self {
        self.origin = Some((runtime, worker));
        self
    }
}

/// A task whose creation event was accepted by the recorder.
#[derive(Debug)]
pub struct MonitoredTask {
    properties: TaskProperties,
    runtime: RuntimeId,
    backtrace: BacktraceCapture,
    recorded: bool,
}

impl MonitoredTask {
    pub(crate) fn unrecorded(id: TaskId, runtime: RuntimeId) -> Self {
        Self {
            properties: TaskProperties::new(id, TypeDescriptorId::unknown()),
            runtime,
            backtrace: BacktraceCapture::Never,
            recorded: false,
        }
    }

    /// Returns the process-wide task identity.
    #[must_use]
    pub const fn id(&self) -> TaskId {
        self.properties.id
    }

    /// Returns the process-monotonic runtime identity.
    #[must_use]
    pub const fn runtime_id(&self) -> RuntimeId {
        self.runtime
    }

    /// Builds a snapshot row with this task's recorded creation properties.
    #[must_use]
    pub fn snapshot(&self, state: crate::snapshot::TaskActivityState) -> Task {
        let mut task = Task::new(self.properties.id, self.properties.descriptor, state);
        task.parent = self.properties.parent;
        task.spawned_at = self.properties.spawned_at;
        task.future_size_bytes = self
            .properties
            .body_size
            .map(|size| u64::try_from(size).expect("the spawn builder already checked that the body size fits in u64"));
        task
    }

    /// Starts one poll on the supplied worker.
    #[must_use]
    pub fn poll_started(&self, worker: &crate::worker::WorkerHandle) -> TaskPoll {
        self.validate_worker(worker);
        worker.task_poll_started(self.id())
    }

    /// Finishes one poll on the supplied worker.
    pub fn poll_finished(&self, worker: &crate::worker::WorkerHandle, poll: TaskPoll) {
        self.validate_worker(worker);
        worker.task_poll_finished(poll);
    }

    /// Records successful task completion.
    pub fn completed(&self, worker: &crate::worker::WorkerHandle) {
        self.outcome(worker, EventKind::TaskCompleted, BacktraceCapture::Never);
    }

    /// Records a task panic.
    pub fn panicked(&self, worker: &crate::worker::WorkerHandle) {
        self.outcome(worker, EventKind::TaskPanicked, self.backtrace);
    }

    /// Records cancellation without worker attribution.
    pub fn canceled(&self) {
        if self.recorded {
            outcome(self.runtime, None, BacktraceCapture::Never, self.id(), EventKind::TaskCanceled);
        }
    }

    fn validate_worker(&self, worker: &crate::worker::WorkerHandle) {
        assert_eq!(
            worker.runtime_id(),
            self.runtime,
            "use a monitored task with a worker from the task runtime"
        );
    }

    fn outcome(&self, worker: &crate::worker::WorkerHandle, kind: EventKind, backtrace: BacktraceCapture) {
        self.validate_worker(worker);
        if self.recorded {
            outcome(self.runtime, Some(worker.id()), backtrace, self.id(), kind);
        }
    }
}

pub(crate) fn spawned(
    runtime: RuntimeId,
    worker: Option<WorkerId>,
    backtrace: BacktraceCapture,
    create: impl FnOnce() -> TaskProperties,
) -> Option<MonitoredTask> {
    let mut assigned = None;
    seismograph::record(EventClass::RuntimeTask, || {
        let mut properties = create();
        let size = u64::try_from(properties.body_size.expect("recorded tasks have an inferred or explicit body size"))
            .ok()
            .and_then(|size| size.checked_add(1))
            .expect("a concrete task body must be smaller than u64::MAX");
        let timestamp = properties.spawned_at.unwrap_or_else(EventTimestamp::now);
        properties.spawned_at = Some(timestamp);
        let origin = properties.origin.filter(|(origin, _)| *origin == runtime);
        let record = Record::runtime(
            timestamp,
            EventKind::TaskSpawned,
            RuntimeEvent {
                runtime_id: runtime,
                worker_id: origin.map(|(_, worker)| worker).or(worker),
                subject_id: properties.id.get(),
                related_id: properties.parent.map_or(0, TaskId::get),
                value_0: properties.descriptor.get(),
                value_1: size,
            },
            backtrace,
        );
        assigned = Some(MonitoredTask {
            properties,
            runtime,
            backtrace,
            recorded: true,
        });
        Some(record)
    });
    assigned
}

pub(crate) fn outcome(runtime: RuntimeId, worker: Option<WorkerId>, backtrace: BacktraceCapture, task: TaskId, kind: EventKind) {
    seismograph::record(EventClass::RuntimeTask, || {
        Some(Record::runtime(
            EventTimestamp::now(),
            kind,
            RuntimeEvent {
                runtime_id: runtime,
                worker_id: worker,
                subject_id: task.get(),
                related_id: 0,
                value_0: 0,
                value_1: 0,
            },
            backtrace,
        ))
    });
}

/// An owned request that a provider moves to its worker and answers once.
///
/// Dropping an unanswered request reports collection failure and releases the
/// worker's request slot. Canceled requests do not send a response.
/// No references to native task storage can escape in the response.
#[derive(Debug)]
pub struct TaskRequest {
    pub(crate) worker: Arc<WorkerControl>,
    pub(crate) sender: mpsc::SyncSender<collection::Batch>,
    pub(crate) canceled: Arc<AtomicBool>,
    pub(crate) limit: usize,
    pub(crate) window: u64,
    pub(crate) answered: bool,
}

impl TaskRequest {
    /// Maximum native tasks to inspect, including tasks without descriptions.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the worker that owns this request.
    #[must_use]
    pub fn worker_id(&self) -> WorkerId {
        self.worker.id
    }

    /// Whether the diagnostic caller stopped waiting for this request.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        // Acquire observes the collector's Release when it stops receiving.
        self.canceled.load(Ordering::Acquire)
    }

    /// Sends owned descriptions without waiting for the diagnostic caller.
    ///
    /// Excess descriptions are truncated and marked incomplete. A canceled
    /// request discards its response. Providers set `last_worker_id` to any
    /// worker when a task belongs to this provider; the response normalizes it
    /// to the responding worker. Measurement data comes from observed polls.
    pub fn respond(mut self, mut tasks: Vec<Task>, mut collection: TaskCollection) {
        if self.is_canceled() {
            return;
        }
        if tasks.len() > self.limit {
            tasks.truncate(self.limit);
            collection = TaskCollection::Truncated;
        }
        let measurements = lock(&self.worker.measurements);
        for task in &mut tasks {
            task.last_worker_id = task.last_worker_id.map(|_| self.worker.id);
            task.metrics = TaskMetrics::default();
            task.metrics_scope = crate::snapshot::TaskMetricsScope::Uncollected;
            if self.window != 0
                && measurements.window == self.window
                && let Some(measured) = measurements.tasks.get(&task.id)
            {
                task.metrics = measured.metrics;
                task.metrics_scope = crate::snapshot::TaskMetricsScope::PollsSince(measured.since);
            }
        }
        drop(measurements);
        self.answered = true;
        self.send(collection::Batch {
            worker: self.worker.id,
            tasks,
            collection,
        });
    }

    fn send(&self, batch: collection::Batch) {
        match self.sender.try_send(batch) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
            Err(mpsc::TrySendError::Full(_)) => unreachable!("one response per worker fits the collection channel"),
        }
    }
}

impl Drop for TaskRequest {
    fn drop(&mut self) {
        if !self.answered && !self.is_canceled() {
            self.send(collection::Batch {
                worker: self.worker.id,
                tasks: Vec::new(),
                collection: TaskCollection::Failed,
            });
        }
        // Release transfers this single pending slot to the next dispatch.
        self.worker.request_pending.store(false, Ordering::Release);
    }
}

/// Token pairing a poll observed by one worker.
///
/// Event-recording changes do not reset requested poll measurements.
/// Dropping an unfinished token clears its worker slot without publishing a duration.
#[derive(Debug)]
#[must_use = "finish the observed poll with WorkerHandle::task_poll_finished, or drop it to cancel observation"]
pub struct TaskPoll {
    pub(crate) task: TaskId,
    pub(crate) worker: Arc<WorkerControl>,
    pub(crate) started_at: Option<EventTimestamp>,
    pub(crate) window: u64,
    pub(crate) active: bool,
}

impl TaskPoll {
    /// Returns the timestamp used by the poll-start event.
    #[must_use]
    pub const fn started_at(&self) -> Option<EventTimestamp> {
        self.started_at
    }
}

impl Drop for TaskPoll {
    fn drop(&mut self) {
        if self.active {
            let _previous = self
                .worker
                .current_task
                .compare_exchange(self.task.get(), 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_descriptors_are_process_wide_across_threads() {
        let local = type_descriptor::<u32>();
        let remote = std::thread::spawn(type_descriptor::<u32>).join().unwrap();

        assert_eq!(remote, local);
        assert_ne!(type_descriptor::<u64>(), local);
    }
}
