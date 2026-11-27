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
    parent: Option<TaskId>,
    body_size: u64,
    enqueued_at: Option<EventTimestamp>,
    recorded: bool,
}

/// An asynchronous task created before it enters a queue.
#[derive(Debug)]
#[must_use = "enqueue the task handle or drop it without recording a lifecycle"]
pub struct TaskHandle {
    data: TaskData,
    origin: Option<WorkerId>,
}

impl TaskHandle {
    pub(crate) fn new(runtime: RuntimeId, backtrace: BacktraceCapture, identity: TaskIdentity, body_size: usize) -> Option<Self> {
        is_enabled(EventClass::RuntimeTask).then(|| Self {
            data: TaskData {
                identity,
                runtime,
                backtrace,
                last_worker: None,
                parent: None,
                body_size: encode_body_size(body_size),
                enqueued_at: None,
                recorded: false,
            },
            origin: None,
        })
    }

    /// Attaches queue context to this task.
    #[must_use]
    pub fn with_context(mut self, origin: Option<crate::WorkerOrigin>, parent: Option<TaskId>) -> Self {
        self.origin = origin.and_then(|origin| origin.worker_for(self.data.runtime));
        self.data.parent = parent;
        self
    }

    /// Records the task entering a queue.
    #[must_use]
    pub fn enqueued(mut self, queue_len: u64) -> QueuedTask {
        seismograph::record(EventClass::RuntimeTask, || {
            let enqueued_at = EventTimestamp::now();
            self.data.enqueued_at = Some(enqueued_at);
            self.data.recorded = true;
            Some(runtime_record(
                enqueued_at,
                EventKind::TaskEnqueued,
                self.data.runtime,
                self.origin,
                self.data.identity.id().get(),
                self.data.parent.map_or(0, TaskId::get),
                self.data.identity.descriptor().get(),
                queue_len,
                self.data.backtrace,
            ))
        });
        let cancel_on_drop = self.data.recorded;
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
    data: TaskData,
    cancel_on_drop: bool,
}

impl QueuedTask {
    /// Returns the caller-assigned task identity.
    #[must_use]
    pub const fn identity(&self) -> TaskIdentity {
        self.data.identity
    }

    /// Activates the task on its first execution worker.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime.
    pub fn activated(self, worker: &super::Worker) -> Task {
        self.activated_with::<true>(worker)
    }

    #[doc(hidden)]
    pub fn activated_with<const RECORDING: bool>(mut self, worker: &super::Worker) -> Task {
        worker.validate_runtime(self.data.runtime);
        if RECORDING && self.data.recorded {
            let activated_at = EventTimestamp::now();
            record(
                activated_at,
                EventKind::TaskMaterialized,
                self.data,
                Some(worker.id()),
                self.data
                    .enqueued_at
                    .map_or(0, |enqueued_at| crate::runtime::duration_nanos(activated_at, enqueued_at)),
                self.data.body_size,
                BacktraceCapture::Never,
            );
        }
        self.cancel_on_drop = false;
        let cancel_on_drop = self.data.recorded;
        Task {
            data: TaskData {
                last_worker: Some(worker.id()),
                ..self.data
            },
            cancel_on_drop,
        }
    }
}

impl Drop for QueuedTask {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            record_cancelled(self.data);
        }
    }
}

/// An activated asynchronous task between polls.
#[derive(Debug)]
#[must_use = "start a poll or drop the task to record cancellation"]
pub struct Task {
    data: TaskData,
    cancel_on_drop: bool,
}

impl Task {
    /// Creates a task with no retained queue lifecycle.
    #[must_use]
    pub fn unrecorded(identity: TaskIdentity, worker: &super::Worker) -> Self {
        Self {
            data: TaskData {
                identity,
                runtime: worker.runtime().id(),
                backtrace: BacktraceCapture::Never,
                last_worker: Some(worker.id()),
                parent: None,
                body_size: 1,
                enqueued_at: None,
                recorded: false,
            },
            cancel_on_drop: false,
        }
    }

    /// Returns the caller-assigned task identity.
    #[must_use]
    pub const fn identity(&self) -> TaskIdentity {
        self.data.identity
    }

    /// Starts one poll.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime.
    pub fn poll_started(self, worker: &super::Worker) -> Poll {
        self.poll_started_with::<true>(worker)
    }

    #[doc(hidden)]
    pub fn poll_started_with<const RECORDING: bool>(mut self, worker: &super::Worker) -> Poll {
        worker.validate_runtime(self.data.runtime);
        let mut started_at = None;
        if RECORDING {
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
        }
        self.cancel_on_drop = false;
        Poll {
            data: TaskData {
                last_worker: Some(worker.id()),
                ..self.data
            },
            worker: worker.id(),
            started_at,
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
    worker: WorkerId,
    started_at: Option<EventTimestamp>,
}

impl Poll {
    /// Finishes the poll and returns the task while it remains pending.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime or did not start this poll.
    pub fn finished(self, worker: &super::Worker, outcome: Option<TaskOutcome>) -> Option<Task> {
        self.finished_with::<true>(worker, outcome)
    }

    #[doc(hidden)]
    pub fn finished_with<const RECORDING: bool>(self, worker: &super::Worker, outcome: Option<TaskOutcome>) -> Option<Task> {
        worker.validate_runtime(self.data.runtime);
        assert_eq!(
            worker.id(),
            self.worker,
            "finish an asynchronous poll on the worker that started it"
        );
        if !RECORDING {
            return outcome.is_none().then(|| Task {
                data: self.data,
                cancel_on_drop: self.data.recorded,
            });
        }
        let Some(started_at) = self.started_at else {
            return outcome.is_none().then(|| Task {
                data: self.data,
                cancel_on_drop: self.data.recorded,
            });
        };
        let finished_at = EventTimestamp::now();
        let backtrace = if outcome.as_ref() == Some(&TaskOutcome::Panicked) {
            self.data.backtrace
        } else {
            BacktraceCapture::Never
        };
        record(
            finished_at,
            EventKind::TaskPollFinished,
            self.data,
            Some(worker.id()),
            crate::runtime::duration_nanos(finished_at, started_at),
            outcome.as_ref().map_or(0, TaskOutcome::event_value),
            backtrace,
        );
        outcome.is_none().then(|| Task {
            data: self.data,
            cancel_on_drop: self.data.recorded,
        })
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
