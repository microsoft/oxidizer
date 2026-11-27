// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::is_enabled;
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, WorkerId};

use crate::{TaskIdentity, TaskOutcome};

#[derive(Clone, Copy, Debug)]
struct TaskData {
    identity: TaskIdentity,
    runtime: RuntimeId,
    backtrace: BacktraceCapture,
    last_worker: Option<WorkerId>,
    recorded: bool,
}

#[derive(Clone, Copy, Debug)]
struct QueueData {
    task: TaskData,
    parent: Option<TaskId>,
    body_size: u64,
    enqueued_at: Option<EventTimestamp>,
}

/// An asynchronous task created before it enters a queue.
#[derive(Debug)]
#[must_use = "enqueue the task handle or drop it without recording a lifecycle"]
pub struct TaskHandle {
    data: QueueData,
    origin: Option<WorkerId>,
}

impl TaskHandle {
    pub(crate) fn new(runtime: RuntimeId, backtrace: BacktraceCapture, identity: TaskIdentity, body_size: usize) -> Option<Self> {
        is_enabled(EventClass::RuntimeTask).then(|| Self {
            data: QueueData {
                task: TaskData {
                    identity,
                    runtime,
                    backtrace,
                    last_worker: None,
                    recorded: false,
                },
                parent: None,
                body_size: encode_body_size(body_size),
                enqueued_at: None,
            },
            origin: None,
        })
    }

    /// Attaches queue context to this task.
    pub fn with_context(mut self, origin: Option<crate::WorkerOrigin>, parent: Option<TaskId>) -> Self {
        self.origin = origin.and_then(|origin| origin.worker_for(self.data.task.runtime));
        self.data.parent = parent;
        self
    }

    /// Records the task entering a queue.
    pub fn enqueued(mut self, queue_len: u64) -> QueuedTask {
        seismograph::record(EventClass::RuntimeTask, || {
            let enqueued_at = EventTimestamp::now();
            self.data.enqueued_at = Some(enqueued_at);
            self.data.task.recorded = true;
            Some(runtime_record(
                enqueued_at,
                EventKind::TaskEnqueued,
                self.data.task.runtime,
                self.origin,
                self.data.task.identity.id().get(),
                self.data.parent.map_or(0, TaskId::get),
                self.data.task.identity.descriptor().get(),
                queue_len,
                self.data.task.backtrace,
            ))
        });
        let cancel_on_drop = self.data.task.recorded;
        QueuedTask {
            data: self.data,
            cancel_on_drop,
        }
    }
}

/// An asynchronous task waiting for its first execution.
#[derive(Debug)]
#[must_use = "activate the queued task or drop it to record cancellation"]
pub struct QueuedTask {
    data: QueueData,
    cancel_on_drop: bool,
}

impl QueuedTask {
    /// Returns the caller-assigned task identity.
    #[must_use]
    pub const fn identity(&self) -> TaskIdentity {
        self.data.task.identity
    }

    /// Activates the task on its first execution worker.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime.
    pub fn activated(mut self, worker: &super::Worker) -> Task {
        worker.validate_runtime(self.data.task.runtime);
        if self.data.task.recorded {
            let activated_at = EventTimestamp::now();
            record(
                activated_at,
                EventKind::TaskMaterialized,
                self.data.task,
                Some(worker.id()),
                self.data
                    .enqueued_at
                    .map_or(0, |enqueued_at| crate::runtime::duration_nanos(activated_at, enqueued_at)),
                self.data.body_size,
                BacktraceCapture::Never,
            );
        }
        self.cancel_on_drop = false;
        let cancel_on_drop = self.data.task.recorded;
        Task {
            data: TaskData {
                last_worker: Some(worker.id()),
                ..self.data.task
            },
            cancel_on_drop,
            retain_on_pending: true,
        }
    }
}

impl Drop for QueuedTask {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            record_cancelled(self.data.task);
        }
    }
}

/// An activated asynchronous task between polls.
#[derive(Debug)]
#[must_use = "start a poll or drop the task to record cancellation"]
pub struct Task {
    data: TaskData,
    cancel_on_drop: bool,
    retain_on_pending: bool,
}

impl Task {
    /// Creates a task with no retained queue lifecycle.
    ///
    /// Its polls are independent. Finishing a pending poll does not return task
    /// state for the next poll.
    pub fn unrecorded(identity: TaskIdentity, worker: &super::Worker) -> Self {
        Self {
            data: TaskData {
                identity,
                runtime: worker.runtime().id(),
                backtrace: BacktraceCapture::Never,
                last_worker: Some(worker.id()),
                recorded: false,
            },
            cancel_on_drop: false,
            retain_on_pending: false,
        }
    }

    /// Returns the caller-assigned task identity.
    #[must_use]
    pub const fn identity(&self) -> TaskIdentity {
        self.data.identity
    }

    /// Starts one poll.
    pub fn poll_started(mut self, worker: &super::Worker) -> Poll {
        let mut started_at = None;
        seismograph::record(EventClass::RuntimeTask, || {
            let at = EventTimestamp::now();
            started_at = Some(at);
            self.data.recorded = true;
            Some(runtime_record(
                at,
                EventKind::TaskPollStarted,
                self.data.runtime,
                Some(worker.id()),
                self.data.identity.id().get(),
                0,
                0,
                0,
                BacktraceCapture::Never,
            ))
        });
        self.cancel_on_drop = false;
        Poll {
            data: TaskData {
                last_worker: Some(worker.id()),
                ..self.data
            },
            started_at,
            retain_on_pending: self.retain_on_pending,
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            record_cancelled(self.data);
        }
    }
}

/// One active asynchronous task poll.
#[derive(Debug)]
#[must_use = "finish the poll to return or complete its task"]
pub struct Poll {
    data: TaskData,
    started_at: Option<EventTimestamp>,
    retain_on_pending: bool,
}

impl Poll {
    /// Finishes the poll and returns the task while it remains pending.
    #[must_use = "a pending poll returns the task for its next poll"]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the affine poll transition consumes its optional terminal outcome"
    )]
    pub fn finished(self, outcome: Option<TaskOutcome>) -> Option<Task> {
        self.record_finished(outcome.as_ref());
        (outcome.is_none() && self.retain_on_pending).then(|| Task {
            data: self.data,
            cancel_on_drop: self.data.recorded,
            retain_on_pending: true,
        })
    }

    fn record_finished(&self, outcome: Option<&TaskOutcome>) {
        let Some(started_at) = self.started_at else {
            return;
        };
        let finished_at = EventTimestamp::now();
        let backtrace = if outcome == Some(&TaskOutcome::Panicked) {
            self.data.backtrace
        } else {
            BacktraceCapture::Never
        };
        record(
            finished_at,
            EventKind::TaskPollFinished,
            self.data,
            self.data.last_worker,
            crate::runtime::duration_nanos(finished_at, started_at),
            outcome.map_or(0, TaskOutcome::event_value),
            backtrace,
        );
    }
}

fn record_cancelled(data: TaskData) {
    if !data.recorded || !is_enabled(EventClass::RuntimeTask) {
        return;
    }
    record(
        EventTimestamp::now(),
        EventKind::TaskCanceled,
        data,
        data.last_worker,
        0,
        0,
        BacktraceCapture::Never,
    );
}

fn encode_body_size(body_size: usize) -> u64 {
    u64::try_from(body_size)
        .ok()
        .and_then(|size| size.checked_add(1))
        .expect("a concrete task body must be smaller than u64::MAX")
}

fn record(
    at: EventTimestamp,
    kind: EventKind,
    data: TaskData,
    worker: Option<WorkerId>,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) {
    if !data.recorded {
        return;
    }
    seismograph::record(EventClass::RuntimeTask, || {
        Some(runtime_record(
            at,
            kind,
            data.runtime,
            worker,
            data.identity.id().get(),
            0,
            value_0,
            value_1,
            backtrace,
        ))
    });
}

#[expect(
    clippy::too_many_arguments,
    reason = "The fixed runtime event payload has a timestamp, two identities, and two numeric values"
)]
fn runtime_record(
    timestamp: EventTimestamp,
    kind: EventKind,
    runtime: RuntimeId,
    worker: Option<WorkerId>,
    subject_id: u64,
    related_id: u64,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) -> Record {
    Record::runtime(
        timestamp,
        kind,
        RuntimeEvent {
            runtime_id: runtime,
            worker_id: worker,
            subject_id,
            related_id,
            value_0,
            value_1,
        },
        backtrace,
    )
}
