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

/// Task-event identities and backtrace policy that implement [`Copy`].
///
/// Obtain this from [`WorkerHandle::task_events`](crate::worker::WorkerHandle::task_events).
/// It owns no registration, counter, task state, or shared handle.
#[derive(Clone, Copy, Debug)]
pub struct EventContext {
    runtime: RuntimeId,
    worker: Option<WorkerId>,
    backtrace: BacktraceCapture,
}

static TYPE_DESCRIPTORS: OnceLock<Mutex<HashMap<TypeId, TypeDescriptorId>>> = OnceLock::new();

/// Returns the process-wide descriptor for one concrete task type.
#[must_use]
pub fn type_descriptor<T: 'static>() -> TypeDescriptorId {
    let mut descriptors = lock(TYPE_DESCRIPTORS.get_or_init(|| Mutex::new(HashMap::new())));
    *descriptors
        .entry(TypeId::of::<T>())
        .or_insert_with(crate::allocate_type_descriptor_id)
}

/// Metadata used to create and describe one task.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskProperties {
    parent: Option<TaskId>,
    body_size: usize,
    spawned_at: Option<EventTimestamp>,
}

impl TaskProperties {
    /// Sets the parent task that was active at submission.
    #[must_use]
    pub const fn parent(mut self, parent: Option<TaskId>) -> Self {
        self.parent = parent;
        self
    }

    /// Sets the task body size before wrapping or type erasure.
    #[must_use]
    pub const fn body_size(mut self, body_size: usize) -> Self {
        self.body_size = body_size;
        self
    }

    /// Sets or clears the task spawn timestamp.
    #[must_use]
    pub const fn spawned_at(mut self, spawned_at: Option<EventTimestamp>) -> Self {
        self.spawned_at = spawned_at;
        self
    }

    /// Applies these properties to a native task description.
    pub fn apply_to(&self, task: &mut Task) {
        task.parent = self.parent;
        task.spawned_at = self.spawned_at;
        task.future_size_bytes = Some(u64::try_from(self.body_size).expect("a task body fits the platform address space"));
    }
}

/// Identity and timestamp assigned while building a task creation event.
///
/// Sampling or a concurrent recording change can omit the event after this
/// metadata is assigned. Keep the identity for inspection and later outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskSpawn {
    /// Process-wide task identity.
    pub id: TaskId,
    /// Concrete task type identity used by the event.
    pub descriptor: TypeDescriptorId,
    /// Timestamp used by the task creation event.
    pub timestamp: EventTimestamp,
}

impl EventContext {
    pub(crate) const fn new(runtime: RuntimeId, worker: Option<WorkerId>, backtrace: BacktraceCapture) -> Self {
        Self {
            runtime,
            worker,
            backtrace,
        }
    }

    /// Returns this context without worker attribution.
    #[must_use]
    pub const fn without_worker(mut self) -> Self {
        self.worker = None;
        self
    }

    /// Returns the worker attributed to events from this context.
    #[must_use]
    pub const fn worker_id(&self) -> Option<WorkerId> {
        self.worker
    }

    /// Returns the runtime attributed to events from this context.
    #[must_use]
    pub const fn runtime_id(&self) -> RuntimeId {
        self.runtime
    }

    /// Builds a task creation event and returns its identity and timestamp.
    ///
    /// Calls `descriptor` only when the recorder runs the event builder.
    /// Disabled or suppressed recording does not call it. Sampling and
    /// concurrent recording changes can still omit the event after it runs.
    /// Reuse the returned timestamp in native task metadata to avoid another
    /// clock read. Measure `body_size` before wrapping or type erasure.
    /// The worker identifies the submission origin, not the execution worker.
    ///
    /// # Panics
    ///
    /// Propagates a panic from `descriptor`. Panics if process task identities
    /// are exhausted or `body_size` cannot be encoded below `u64::MAX`.
    #[must_use]
    pub fn spawned(&self, descriptor: impl FnOnce() -> TypeDescriptorId, properties: &TaskProperties) -> Option<TaskSpawn> {
        let mut assigned = None;
        seismograph::record(EventClass::RuntimeTask, || {
            let size = u64::try_from(properties.body_size)
                .ok()
                .and_then(|size| size.checked_add(1))
                .expect("a concrete task body must be smaller than u64::MAX");
            let id = crate::allocate_task_id();
            let descriptor = descriptor();
            let timestamp = properties.spawned_at.unwrap_or_else(EventTimestamp::now);
            assigned = Some(TaskSpawn { id, descriptor, timestamp });
            Some(Record::runtime(
                timestamp,
                EventKind::TaskSpawned,
                RuntimeEvent {
                    runtime_id: self.runtime,
                    worker_id: self.worker,
                    subject_id: id.get(),
                    related_id: properties.parent.map_or(0, TaskId::get),
                    value_0: descriptor.get(),
                    value_1: size,
                },
                self.backtrace,
            ))
        });
        assigned
    }

    /// Emits a real successful outcome without retaining task state.
    #[inline]
    pub fn completed(&self, task: TaskId) {
        self.outcome(task, EventKind::TaskCompleted, BacktraceCapture::Never);
    }

    /// Emits a real task panic with the configured backtrace policy.
    #[inline]
    pub fn panicked(&self, task: TaskId) {
        self.outcome(task, EventKind::TaskPanicked, self.backtrace);
    }

    /// Emits cancellation without retaining task state.
    #[inline]
    pub fn canceled(&self, task: TaskId) {
        self.outcome(task, EventKind::TaskCanceled, BacktraceCapture::Never);
    }

    fn outcome(&self, task: TaskId, kind: EventKind, backtrace: BacktraceCapture) {
        seismograph::record(EventClass::RuntimeTask, || {
            Some(Record::runtime(
                EventTimestamp::now(),
                kind,
                RuntimeEvent {
                    runtime_id: self.runtime,
                    worker_id: self.worker,
                    subject_id: task.get(),
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                backtrace,
            ))
        });
    }
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
