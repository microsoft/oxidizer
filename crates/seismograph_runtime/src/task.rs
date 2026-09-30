// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task lifecycle instrumentation for logical runtimes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::RecordingSession;
use seismograph::recorder::event::{EventClass, EventTimestamp};
use seismograph::recorder::runtime::TaskId;

use crate::worker::WorkerHandle;
use crate::{TaskControl, duration_nanos};

/// Cheap task handle used to correlate wake notifications with subsequent polls.
#[derive(Clone, Debug)]
pub struct TaskHandle {
    pub(crate) task: Arc<TaskControl>,
}

impl TaskHandle {
    pub(crate) fn new(task: Arc<TaskControl>) -> Self {
        Self { task }
    }

    /// Returns this task's process-monotonic identity.
    #[must_use]
    pub fn id(&self) -> TaskId {
        self.task.id
    }

    /// Marks the task ready to run, retaining only the first wake before its next poll.
    #[inline]
    pub fn woken(&self) {
        if seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask) {
            self.task.activity.woken(&self.task);
        } else {
            self.task.wake_timestamp();
        }
    }

    /// Starts a poll and records how long the task waited after becoming ready.
    #[inline]
    pub fn poll_started(&self, worker: &WorkerHandle) -> TaskPoll {
        if seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask) {
            return self.poll_started_recorded(worker);
        }
        let (started_at, ready_since) = self.task.poll_timestamps();
        self.update_poll_metrics(worker, started_at, ready_since);
        worker.task_poll_started_at(self.id(), started_at, None)
    }

    #[cold]
    fn poll_started_recorded(&self, worker: &WorkerHandle) -> TaskPoll {
        let (started_at, ready_since, queued_since, session) = self.task.activity.poll_started(&self.task, worker.id());
        self.update_poll_metrics(worker, started_at, ready_since);
        worker.task_poll_started_recorded(self.id(), started_at, queued_since, session)
    }

    // The assembly probe confirms these factored lifetime operations otherwise
    // become extra cross-crate calls in the existing disabled hot path.
    #[inline]
    fn update_poll_metrics(&self, worker: &WorkerHandle, started_at: EventTimestamp, ready_since: u64) {
        self.task.last_worker_id.store(worker.id().get(), Ordering::Release);
        let previous_poll_finished = self.task.last_poll_finished_at.swap(0, Ordering::AcqRel);
        if previous_poll_finished != 0 {
            let resume_nanos = duration_nanos(started_at, EventTimestamp::from_ticks(previous_poll_finished));
            self.task.resume_count.fetch_add(1, Ordering::Relaxed);
            self.task.resume_duration_nanos.fetch_add(resume_nanos, Ordering::Relaxed);
            self.task.max_resume_duration_nanos.fetch_max(resume_nanos, Ordering::Relaxed);
        }

        let ready_since = (ready_since != 0).then_some(EventTimestamp::from_ticks(ready_since));
        if let Some(ready_since) = ready_since {
            let ready_wait_nanos = duration_nanos(started_at, ready_since);
            self.task.ready_wait_count.fetch_add(1, Ordering::Relaxed);
            self.task.ready_wait_duration_nanos.fetch_add(ready_wait_nanos, Ordering::Relaxed);
            self.task
                .max_ready_wait_duration_nanos
                .fetch_max(ready_wait_nanos, Ordering::Relaxed);
        }
    }

    /// Finishes a poll and updates this task's lifetime counters.
    #[inline]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "consuming the token prevents callers from finishing one poll twice"
    )]
    pub fn poll_finished(&self, worker: &WorkerHandle, poll: TaskPoll) {
        worker.task_poll_finished_with_control(&poll, Some(&self.task));
    }
}

impl TaskControl {
    #[inline]
    pub(crate) fn wake_timestamp(&self) -> EventTimestamp {
        let ready_since = EventTimestamp::now().ticks().max(1);
        let _already_ready = self
            .ready_since
            .compare_exchange(0, ready_since, Ordering::Release, Ordering::Relaxed);
        EventTimestamp::from_ticks(ready_since)
    }

    #[inline]
    pub(crate) fn poll_timestamps(&self) -> (EventTimestamp, u64) {
        let ready_since = self.ready_since.swap(0, Ordering::AcqRel);
        (EventTimestamp::now(), ready_since)
    }
}

/// Token pairing a task poll's start and finish events.
#[derive(Debug)]
#[must_use = "finish the poll with WorkerHandle::task_poll_finished"]
pub struct TaskPoll {
    pub(crate) task_id: TaskId,
    pub(crate) started_at: EventTimestamp,
    pub(crate) session: Option<RecordingSession>,
}
