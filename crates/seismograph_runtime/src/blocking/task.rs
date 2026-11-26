// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, WorkerId};

use crate::{TaskIdentity, TaskOutcome};

#[derive(Clone, Copy, Debug)]
struct TaskData {
    identity: TaskIdentity,
    runtime: RuntimeId,
    backtrace: BacktraceCapture,
}

/// A blocking task waiting in its queue.
#[derive(Debug)]
#[must_use = "start the queued task or drop it to record cancellation"]
pub struct QueuedTask {
    data: TaskData,
    enqueued_at: EventTimestamp,
    body_size: u64,
    cancel_on_drop: bool,
}

impl QueuedTask {
    pub(crate) fn enqueue(
        runtime: RuntimeId,
        backtrace: BacktraceCapture,
        origin: Option<crate::WorkerOrigin>,
        identity: TaskIdentity,
        parent: Option<TaskId>,
        body_size: usize,
        queue_len: u64,
    ) -> Option<Self> {
        let origin = origin.map(|origin| origin.worker_for(runtime));
        let mut queued = None;
        seismograph::record(EventClass::RuntimeTask, || {
            let body_size = encode_body_size(body_size);
            let enqueued_at = EventTimestamp::now();
            queued = Some(Self {
                data: TaskData {
                    identity,
                    runtime,
                    backtrace,
                },
                enqueued_at,
                body_size,
                cancel_on_drop: true,
            });
            Some(runtime_record(
                enqueued_at,
                EventKind::TaskEnqueued,
                runtime,
                origin,
                identity.id().get(),
                parent.map_or(0, TaskId::get),
                identity.descriptor().get(),
                queue_len,
                backtrace,
            ))
        });
        queued
    }

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
        let started_at = EventTimestamp::now();
        record(
            started_at,
            EventKind::TaskMaterialized,
            self.data,
            Some(worker.id()),
            crate::duration_nanos(started_at, self.enqueued_at),
            self.body_size,
            BacktraceCapture::Never,
        );
        self.cancel_on_drop = false;
        Execution {
            data: self.data,
            worker: worker.id(),
            started_at,
            finish_on_drop: true,
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
    started_at: EventTimestamp,
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
            crate::duration_nanos(finished_at, self.started_at),
            outcome,
            backtrace,
        );
        self.finish_on_drop = false;
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        if self.finish_on_drop {
            let finished_at = EventTimestamp::now();
            record_finished(
                self.data,
                Some(self.worker),
                finished_at,
                crate::duration_nanos(finished_at, self.started_at),
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
