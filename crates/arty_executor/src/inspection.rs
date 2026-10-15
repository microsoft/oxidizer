// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::marker::PhantomData;

/// Scheduling state read from the executor's own task queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    /// The task has been submitted or notified and is awaiting a poll.
    Ready,
    /// The task is pending without an observed notification.
    Waiting,
    /// A requested poll observer is about to poll this task.
    Running,
}

/// Optional poll instrumentation owned once by an executor, not by every task.
///
/// The executor checks [`Self::enabled`] once per cycle. Disabled observers do
/// not describe tasks, read clocks, or receive per-poll callbacks. Descriptions
/// passed to `poll_started` are owned values; references to futures do not escape.
/// Start and finish callbacks are paired only for polls whose inspector returned
/// a description. Tasks without descriptions invoke neither callback.
/// Description panics skip that observation and invoke [`Self::inspection_failed`];
/// the task still runs. The runtime must report or otherwise handle that error.
/// Implementations must not panic, just like the executor's owner waker: these
/// callbacks belong to the runtime, not to an external monitor or exporter.
pub trait PollObserver: std::fmt::Debug {
    /// Whether the next cycle needs observation.
    fn enabled(&self) -> bool;

    /// Receives a task description before its future is polled.
    fn poll_started(&self, description: &dyn Any);

    /// Finishes the observation after polling, including tasks that completed.
    fn poll_finished(&self, completed: bool);

    /// Reports a failed task description without treating it as a task panic.
    ///
    /// No finish callback is issued for this observation. Discard any partially
    /// started observation state and report the diagnostic failure to its consumer.
    fn inspection_failed(&self, error: InspectionError);
}

/// Whether a bounded inspection visited every live task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionStatus {
    /// Every live task was visited.
    Complete,
    /// The task limit was reached; more live tasks exist.
    Truncated,
}

/// An inspection could not safely borrow the executor's task storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionError {
    /// Inspection was requested during execution, another inspection, or a contended wake.
    Busy,
    /// The executor has been dropped.
    Disconnected,
    /// A description function or inspection visitor panicked.
    Panicked,
}

impl std::fmt::Display for InspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "the executor is busy; inspect between execution cycles",
            Self::Disconnected => "the inspected executor has been dropped",
            Self::Panicked => "the task description or inspection visitor panicked",
        })
    }
}

impl std::error::Error for InspectionError {}

pub(crate) trait Inspector<F> {
    fn inspect(&self, future: &F, state: TaskState, visitor: &mut dyn FnMut(&dyn Any));
}

impl<F> Inspector<F> for () {
    fn inspect(&self, _future: &F, _state: TaskState, _visitor: &mut dyn FnMut(&dyn Any)) {}
}

#[derive(Debug)]
pub(crate) struct InspectWith<I, D> {
    inspect: I,
    _data: PhantomData<fn() -> D>,
}

impl<I, D> InspectWith<I, D> {
    pub(crate) fn new(inspect: I) -> Self {
        Self {
            inspect,
            _data: PhantomData,
        }
    }
}

impl<F, I, D> Inspector<F> for InspectWith<I, D>
where
    I: Fn(&F, TaskState) -> Option<D>,
    D: 'static,
{
    fn inspect(&self, future: &F, state: TaskState, visitor: &mut dyn FnMut(&dyn Any)) {
        if let Some(data) = (self.inspect)(future, state) {
            visitor(&data);
        }
    }
}
