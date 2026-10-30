// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "inspection")]
use std::any::Any;
#[cfg(feature = "inspection")]
use std::marker::PhantomData;

/// Scheduling state read from native executor storage.
#[cfg(feature = "inspection")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    /// Submitted or notified and awaiting a poll.
    Ready,
    /// Pending without an outstanding wake notification.
    Waiting,
    /// About to be polled by an explicitly observed execution cycle.
    Running,
}

/// Completeness of a bounded inspection.
#[cfg(feature = "inspection")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionStatus {
    /// All native live tasks were inspected.
    Complete,
    /// The work limit was reached.
    Truncated,
}

/// Failure to inspect native task storage.
#[cfg(feature = "inspection")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionError {
    /// The work limit was zero.
    InvalidLimit,
    /// Submission storage is borrowed by another operation.
    Busy,
    /// An inspector or visitor panicked.
    Panicked,
}

#[cfg(feature = "inspection")]
impl std::fmt::Display for InspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidLimit => "the task inspection limit must be greater than zero",
            Self::Busy => "native task storage is busy",
            Self::Panicked => "the task inspector or visitor panicked",
        })
    }
}

#[cfg(feature = "inspection")]
impl std::error::Error for InspectionError {}

/// Runtime-owned callbacks used only by an explicitly observed execution cycle.
///
/// Start and finish callbacks are paired only when the task supplies a description.
/// These callbacks must not panic. Diagnostic consumers must receive owned results
/// separately, not run on the executor's polling thread.
#[cfg(feature = "inspection")]
pub trait PollObserver: std::fmt::Debug {
    /// Receives a description immediately before the task is polled.
    fn poll_started(&self, description: &dyn Any);
    /// Finishes an observed poll, including a task that completed.
    fn poll_finished(&self, completed: bool);
    /// Reports an inspector panic without preventing the task from running.
    fn inspection_failed(&self, error: InspectionError);
}

pub(crate) trait Inspector<F>: 'static {
    #[cfg(feature = "inspection")]
    fn inspect(&self, future: &F, state: TaskState, visitor: &mut dyn FnMut(&dyn Any));
}

impl<F> Inspector<F> for () {
    #[cfg(feature = "inspection")]
    fn inspect(&self, _future: &F, _state: TaskState, _visitor: &mut dyn FnMut(&dyn Any)) {}
}

#[cfg(feature = "inspection")]
pub(crate) struct InspectWith<I, D> {
    inspect: I,
    _description: PhantomData<fn() -> D>,
}

#[cfg(feature = "inspection")]
impl<I, D> InspectWith<I, D> {
    pub(crate) const fn new(inspect: I) -> Self {
        Self {
            inspect,
            _description: PhantomData,
        }
    }
}

#[cfg(feature = "inspection")]
impl<F, I, D> Inspector<F> for InspectWith<I, D>
where
    I: Fn(&F, TaskState) -> Option<D> + 'static,
    D: 'static,
{
    fn inspect(&self, future: &F, state: TaskState, visitor: &mut dyn FnMut(&dyn Any)) {
        if let Some(description) = (self.inspect)(future, state) {
            visitor(&description);
        }
    }
}
