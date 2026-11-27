// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use seismograph_runtime::asynchronous::{Poll, QueuedTask, Task};
use seismograph_runtime::{TaskIdentity, TaskOutcome};

use crate::ExecutorShared;

pub(crate) trait MonitoringState: Debug + 'static {
    type SilentPoll;
    type RecordedPoll;

    fn poll_silent_started(&mut self, identity: TaskIdentity, shared: &ExecutorShared) -> Self::SilentPoll;

    fn poll_silent_finished(&mut self, poll: Self::SilentPoll, outcome: Option<TaskOutcome>);

    fn poll_recorded_started(&mut self, identity: TaskIdentity, shared: &ExecutorShared) -> Self::RecordedPoll;

    fn poll_recorded_finished(&mut self, poll: Self::RecordedPoll, outcome: Option<TaskOutcome>);
}

#[derive(Debug, Default)]
pub(crate) struct Unmonitored;

impl MonitoringState for Unmonitored {
    type SilentPoll = ();
    type RecordedPoll = Poll;

    fn poll_silent_started(&mut self, _identity: TaskIdentity, _shared: &ExecutorShared) -> Self::SilentPoll {}

    fn poll_silent_finished(&mut self, (): Self::SilentPoll, _outcome: Option<TaskOutcome>) {}

    fn poll_recorded_started(&mut self, identity: TaskIdentity, shared: &ExecutorShared) -> Self::RecordedPoll {
        let worker = shared.monitoring().expect("recorded executor polls require a monitoring worker");
        Task::unrecorded(identity, worker).poll_started(worker)
    }

    fn poll_recorded_finished(&mut self, poll: Self::RecordedPoll, outcome: Option<TaskOutcome>) {
        let retained = poll.finished(outcome);
        debug_assert!(retained.is_none(), "an unrecorded task poll must not retain lifecycle state");
    }
}

#[derive(Debug)]
pub(crate) enum Monitored {
    Dormant,
    Queued(QueuedTask),
    Active(Task),
    Polling(Poll),
}

impl Monitored {
    pub(crate) const fn new(queued: QueuedTask) -> Self {
        Self::Queued(queued)
    }

    fn discard(&mut self) {
        let state = std::mem::replace(self, Self::Dormant);
        // This cycle is silent. Do not let a concurrent policy change turn
        // token cleanup into a cancellation event.
        let _state = std::mem::ManuallyDrop::new(state);
    }
}

impl MonitoringState for Monitored {
    type SilentPoll = ();
    type RecordedPoll = ();

    fn poll_silent_started(&mut self, _identity: TaskIdentity, _shared: &ExecutorShared) -> Self::SilentPoll {
        if matches!(self, Self::Queued(_)) {
            self.discard();
        }
    }

    fn poll_silent_finished(&mut self, (): Self::SilentPoll, outcome: Option<TaskOutcome>) {
        if outcome.is_some() {
            self.discard();
        }
    }

    fn poll_recorded_started(&mut self, identity: TaskIdentity, shared: &ExecutorShared) -> Self::RecordedPoll {
        let worker = shared.monitoring().expect("recorded executor polls require a monitoring worker");
        let task = match std::mem::replace(self, Self::Dormant) {
            Self::Dormant => Task::unrecorded(identity, worker),
            Self::Queued(queued) => queued.activated(worker),
            Self::Active(task) => task,
            Self::Polling(_) => unreachable!("a task cannot start another poll before finishing its current poll"),
        };
        *self = Self::Polling(task.poll_started(worker));
    }

    fn poll_recorded_finished(&mut self, (): Self::RecordedPoll, outcome: Option<TaskOutcome>) {
        let Self::Polling(poll) = std::mem::replace(self, Self::Dormant) else {
            unreachable!("only a polling task can finish a poll");
        };
        if let Some(task) = poll.finished(outcome) {
            *self = Self::Active(task);
        }
    }
}
