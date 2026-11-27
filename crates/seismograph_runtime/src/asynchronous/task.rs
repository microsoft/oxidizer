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
    pub(crate) fn new(
        runtime: RuntimeId,
        backtrace: BacktraceCapture,
        origin: Option<crate::WorkerOrigin>,
        identity: TaskIdentity,
        parent: Option<TaskId>,
        body_size: usize,
    ) -> Self {
        Self {
            data: TaskData {
                identity,
                runtime,
                backtrace,
                last_worker: None,
                parent,
                body_size: encode_body_size(body_size),
                enqueued_at: None,
                recorded: false,
            },
            origin: origin.map(|origin| origin.worker_for(runtime)),
        }
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

    /// Returns the timestamp used by the queue event.
    #[must_use]
    pub const fn enqueued_at(&self) -> Option<EventTimestamp> {
        self.data.enqueued_at
    }

    /// Describes the task from native executor storage.
    #[must_use]
    pub fn snapshot(&self, state: crate::snapshot::TaskActivityState) -> crate::snapshot::Task {
        snapshot(self.data, state)
    }

    /// Activates the task on its first execution worker.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime.
    pub fn activated(mut self, worker: &super::Worker) -> Task {
        worker.validate_runtime(self.data.runtime);
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
        self.cancel_on_drop = false;
        Task {
            data: TaskData {
                last_worker: Some(worker.id()),
                ..self.data
            },
            cancel_on_drop: true,
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
    /// Returns the caller-assigned task identity.
    #[must_use]
    pub const fn identity(&self) -> TaskIdentity {
        self.data.identity
    }

    /// Describes the task from native executor storage.
    #[must_use]
    pub fn snapshot(&self, state: crate::snapshot::TaskActivityState) -> crate::snapshot::Task {
        snapshot(self.data, state)
    }

    /// Starts one poll.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime.
    pub fn poll_started(mut self, worker: &super::Worker) -> Poll {
        worker.validate_runtime(self.data.runtime);
        let started_at = EventTimestamp::now();
        let window = worker.begin_poll(self.data.identity.id());
        record(
            started_at,
            EventKind::TaskPollStarted,
            self.data,
            Some(worker.id()),
            0,
            0,
            BacktraceCapture::Never,
        );
        self.cancel_on_drop = false;
        Poll {
            data: TaskData {
                last_worker: Some(worker.id()),
                ..self.data
            },
            worker: worker.id(),
            started_at,
            window,
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
    started_at: EventTimestamp,
    window: u64,
}

impl Poll {
    /// Finishes the poll and returns the task while it remains pending.
    ///
    /// # Panics
    ///
    /// Panics if `worker` belongs to another runtime or did not start this poll.
    pub fn finished(self, worker: &super::Worker, outcome: Option<TaskOutcome>) -> Option<Task> {
        worker.validate_runtime(self.data.runtime);
        assert_eq!(
            worker.id(),
            self.worker,
            "finish an asynchronous poll on the worker that started it"
        );
        let finished_at = EventTimestamp::now();
        worker.finish_poll(self.data.identity.id(), self.window, self.started_at, finished_at);
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
            crate::runtime::duration_nanos(finished_at, self.started_at),
            outcome.as_ref().map_or(0, TaskOutcome::event_value),
            backtrace,
        );
        outcome.is_none().then_some(Task {
            data: self.data,
            cancel_on_drop: true,
        })
    }
}

fn record_cancelled(data: TaskData) {
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

fn snapshot(data: TaskData, state: crate::snapshot::TaskActivityState) -> crate::snapshot::Task {
    let mut task = crate::snapshot::Task::new(data.identity.id(), data.identity.descriptor(), state);
    task.parent = data.parent;
    task.spawned_at = data.enqueued_at;
    task.future_size_bytes = Some(data.body_size.saturating_sub(1));
    task
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
