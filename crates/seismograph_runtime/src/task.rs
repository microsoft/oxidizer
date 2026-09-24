// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task lifecycle instrumentation for logical runtimes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use seismograph::recorder::event::EventTimestamp;
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
        let ready_since = EventTimestamp::now().ticks().max(1);
        // Release publishes the wake timestamp to the worker that acquires it
        // before polling; a failed comparison does not consume any data.
        let _already_ready = self
            .task
            .ready_since
            .compare_exchange(0, ready_since, Ordering::Release, Ordering::Relaxed);
    }

    /// Starts a poll and records how long the task waited after becoming ready.
    #[inline]
    pub fn poll_started(&self, worker: &WorkerHandle) -> TaskPoll {
        // AcqRel consumes the published first-wake timestamp and resets the
        // task for the next wake-to-poll interval.
        let ready_since = self.task.ready_since.swap(0, Ordering::AcqRel);
        let started_at = EventTimestamp::now();
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
        worker.task_poll_started_at(self.id(), started_at, ready_since)
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

/// Token pairing a task poll's start and finish events.
#[derive(Debug)]
#[must_use = "finish the poll with WorkerHandle::task_poll_finished"]
pub struct TaskPoll {
    pub(crate) task_id: TaskId,
    pub(crate) started_at: EventTimestamp,
}
