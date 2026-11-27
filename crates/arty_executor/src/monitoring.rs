// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::TaskOutcome;
use seismograph_runtime::asynchronous::{Poll, QueuedTask, Task, Worker};

impl TaskMonitoring {
    pub(crate) fn new(queued: Option<QueuedTask>) -> Self {
        queued.map_or(Self::Dormant, Self::Queued)
    }

    pub(crate) fn belongs_to(&self, worker: &Worker) -> bool {
        match self {
            Self::Dormant => true,
            Self::Queued(task) => task.belongs_to(worker),
            Self::Active(_) | Self::Polling(_) => true,
        }
    }

    pub(crate) fn poll_started<const RECORDING: bool>(self, identity: seismograph_runtime::TaskIdentity, worker: &Worker) -> Self {
        let task = match self {
            Self::Dormant => Task::unrecorded(identity, worker),
            Self::Queued(queued) => queued.activated_with::<RECORDING>(worker),
            Self::Active(task) => task,
            Self::Polling(_) => unreachable!("a task cannot start another poll before finishing its current poll"),
        };
        Self::Polling(task.poll_started_with::<RECORDING>(worker))
    }

    pub(crate) fn poll_finished<const RECORDING: bool>(self, worker: &Worker, outcome: Option<TaskOutcome>) -> Option<Self> {
        let Self::Polling(poll) = self else {
            unreachable!("only a polling task can finish a poll");
        };
        poll.finished_with::<RECORDING>(worker, outcome).map(Self::Active)
    }
}

#[derive(Debug)]
pub(crate) enum TaskMonitoring {
    Dormant,
    Queued(QueuedTask),
    Active(Task),
    Polling(Poll),
}
