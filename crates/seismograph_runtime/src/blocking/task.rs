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
    parent: Option<TaskId>,
    body_size: u64,
    enqueued_at: Option<EventTimestamp>,
    recorded: bool,
}

/// A blocking task created before it enters a queue.
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

/// A blocking task waiting in its queue.
#[derive(Debug)]
#[must_use = "start the queued task or drop it to record cancellation"]
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

    /// Starts execution on a blocking worker.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime.
    pub fn started(mut self, worker: &super::Worker) -> Execution {
        worker.validate_runtime(self.data.runtime);
        let started_at = (self.data.recorded && is_enabled(EventClass::RuntimeTask)).then(EventTimestamp::now);
        if let Some(started_at) = started_at {
            record(
                started_at,
                EventKind::TaskMaterialized,
                self.data,
                Some(worker.id()),
                self.data
                    .enqueued_at
                    .map_or(0, |enqueued_at| crate::runtime::duration_nanos(started_at, enqueued_at)),
                self.data.body_size,
                BacktraceCapture::Never,
            );
        }
        self.cancel_on_drop = false;
        Execution {
            data: self.data,
            worker: worker.id(),
            started_at,
            finish_on_drop: self.data.recorded,
        }
    }
}

impl Drop for QueuedTask {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            record_finished(
                self.data,
                None,
                EventTimestamp::now(),
                0,
                TaskOutcome::Cancelled,
                BacktraceCapture::Never,
            );
        }
    }
}

/// One active blocking task execution.
#[derive(Debug)]
#[must_use = "finish the blocking execution"]
pub struct Execution {
    data: TaskData,
    worker: WorkerId,
    started_at: Option<EventTimestamp>,
    finish_on_drop: bool,
}

impl Execution {
    /// Returns the caller-assigned task identity.
    #[must_use]
    pub const fn identity(&self) -> TaskIdentity {
        self.data.identity
    }

    /// Records the task outcome and execution duration.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime or did not start this task.
    pub fn finished(mut self, worker: &super::Worker, outcome: TaskOutcome) {
        worker.validate_runtime(self.data.runtime);
        assert_eq!(worker.id(), self.worker, "finish a blocking task on the worker that started it");
        let Some(started_at) = self.started_at else {
            self.finish_on_drop = false;
            return;
        };
        if !is_enabled(EventClass::RuntimeTask) {
            self.finish_on_drop = false;
            return;
        }
        let finished_at = EventTimestamp::now();
        let backtrace = if outcome == TaskOutcome::Panicked {
            self.data.backtrace
        } else {
            BacktraceCapture::Never
        };
        record_finished(
            self.data,
            Some(worker.id()),
            finished_at,
            crate::runtime::duration_nanos(finished_at, started_at),
            outcome,
            backtrace,
        );
        self.finish_on_drop = false;
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        if self.finish_on_drop {
            if !is_enabled(EventClass::RuntimeTask) {
                return;
            }
            let Some(started_at) = self.started_at else {
                return;
            };
            let finished_at = EventTimestamp::now();
            record_finished(
                self.data,
                Some(self.worker),
                finished_at,
                crate::runtime::duration_nanos(finished_at, started_at),
                TaskOutcome::Cancelled,
                BacktraceCapture::Never,
            );
        }
    }
}

fn record_finished(
    data: TaskData,
    worker: Option<WorkerId>,
    at: EventTimestamp,
    duration: u64,
    outcome: TaskOutcome,
    backtrace: BacktraceCapture,
) {
    if !data.recorded {
        return;
    }
    let kind = match outcome {
        TaskOutcome::Completed => EventKind::TaskCompleted,
        TaskOutcome::Cancelled => EventKind::TaskCanceled,
        TaskOutcome::Panicked => EventKind::TaskPanicked,
    };
    record(at, kind, data, worker, duration, 0, backtrace);
}

fn encode_body_size(body_size: usize) -> u64 {
    u64::try_from(body_size)
        .ok()
        .and_then(|size| size.checked_add(1))
        .expect("a concrete blocking task body must be smaller than u64::MAX")
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
