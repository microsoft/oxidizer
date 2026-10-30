// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native-storage requests, task lifecycle events, and observed poll tokens.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

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

impl EventContext {
    pub(crate) const fn new(runtime: RuntimeId, worker: Option<WorkerId>, backtrace: BacktraceCapture) -> Self {
        Self {
            runtime,
            worker,
            backtrace,
        }
    }

    /// Assigns an identity and emits task creation only when recording is enabled.
    ///
    /// Returns `None` when the recorder does not run the event builder. Sampling
    /// or a concurrent policy change can still omit the event after assigning
    /// an identity. Keep that identity for native inspection and later outcomes.
    /// Measure `body_size` before runtime wrapping or type erasure.
    ///
    /// # Panics
    ///
    /// Panics if process task identities are exhausted or `body_size` cannot be
    /// encoded as a value smaller than `u64::MAX`.
    #[must_use]
    pub fn spawned(&self, descriptor: TypeDescriptorId, parent: Option<TaskId>, body_size: usize) -> Option<TaskId> {
        let mut assigned = None;
        seismograph::record(EventClass::RuntimeTask, || {
            let size = u64::try_from(body_size)
                .ok()
                .and_then(|size| size.checked_add(1))
                .expect("a concrete task body must be smaller than u64::MAX");
            let id = crate::allocate_task_id();
            assigned = Some(id);
            Some(Record::runtime(
                EventTimestamp::now(),
                EventKind::TaskSpawned,
                RuntimeEvent {
                    runtime_id: self.runtime,
                    worker_id: self.worker,
                    subject_id: id.get(),
                    related_id: parent.map_or(0, TaskId::get),
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
/// Dropping an unanswered request releases the worker's request slot. No
/// references to native task storage can escape in the response.
#[derive(Debug)]
pub struct TaskRequest {
    pub(crate) worker: Arc<WorkerControl>,
    pub(crate) sender: mpsc::SyncSender<collection::Batch>,
    pub(crate) canceled: Arc<AtomicBool>,
    pub(crate) limit: usize,
    pub(crate) window: u64,
}

impl TaskRequest {
    /// Maximum native tasks to inspect, including tasks without descriptions.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Whether the diagnostic caller stopped waiting for this request.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    /// Sends owned descriptions without waiting for the diagnostic caller.
    ///
    /// Excess descriptions are truncated and marked incomplete. A canceled
    /// request discards its response. Measurement data comes from observed polls,
    /// not from values supplied by the provider.
    pub fn respond(self, mut tasks: Vec<Task>, mut collection: TaskCollection) {
        if self.is_canceled() {
            return;
        }
        if tasks.len() > self.limit {
            tasks.truncate(self.limit);
            collection = TaskCollection::Truncated;
        }
        let measurements = lock(&self.worker.measurements);
        for task in &mut tasks {
            task.last_worker_id = Some(self.worker.id);
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
        match self.sender.try_send(collection::Batch {
            worker: self.worker.id,
            tasks,
            collection,
        }) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
            Err(mpsc::TrySendError::Full(_)) => unreachable!("one response per worker fits the collection channel"),
        }
    }
}

impl Drop for TaskRequest {
    fn drop(&mut self) {
        self.worker.request_pending.store(false, Ordering::Release);
    }
}

/// Token pairing a poll observed by one worker.
///
/// Its timing-window identity is private and is unrelated to recorder epochs.
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
