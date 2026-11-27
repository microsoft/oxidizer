// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::TaskOutcome;
use seismograph_runtime::asynchronous::{Poll, QueuedTask, Task, Worker};

impl TaskMonitoring {
    pub(crate) fn new(queued: Option<QueuedTask>) -> Self {
        queued.map_or(Self::Dormant, Self::Queued)
    }

    pub(crate) fn poll_started(self, identity: seismograph_runtime::TaskIdentity, worker: &Worker) -> Self {
        let task = match self {
            Self::Dormant => Task::unrecorded(identity, worker),
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

}

#[derive(Debug)]
pub(crate) enum TaskMonitoring {
    Dormant,
    Queued(QueuedTask),
    Active(Task),
    Polling(Poll),
}
