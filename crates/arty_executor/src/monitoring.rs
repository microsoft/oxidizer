// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::TaskOutcome;
use seismograph_runtime::asynchronous::{Poll, QueuedTask, Task, Worker};
use seismograph_runtime::snapshot::{Task as TaskSnapshot, TaskActivityState};

impl TaskMonitoring {
    pub(crate) const fn new(queued: QueuedTask) -> Self {
        Self::Queued(queued)
    }

    pub(crate) fn poll_started(self, worker: &Worker) -> Self {
        let task = match self {
            Self::Queued(queued) => queued.activated(worker),
            Self::Active(task) => task,
            Self::Polling(_) => unreachable!("a task cannot start another poll before finishing its current poll"),
        };
        Self::Polling(task.poll_started(worker))
    }

    pub(crate) fn poll_finished(self, worker: &Worker, outcome: Option<TaskOutcome>) -> Option<Self> {
        let Self::Polling(poll) = self else {
            unreachable!("only a polling task can finish a poll");
        };
        poll.finished(worker, outcome).map(Self::Active)
    }

    pub(crate) fn describe(&self, state: TaskActivityState) -> Option<TaskSnapshot> {
        match self {
            Self::Queued(task) => Some(task.snapshot(state)),
            Self::Active(task) => Some(task.snapshot(state)),
            Self::Polling(_) => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum TaskMonitoring {
    Queued(QueuedTask),
    Active(Task),
    Polling(Poll),
}
