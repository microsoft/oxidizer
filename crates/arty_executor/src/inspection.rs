// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::marker::PhantomData;
use std::task::Poll;

/// Scheduling state read from native executor storage.
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionStatus {
    /// All native live tasks were inspected.
    Complete,
    /// The work limit was reached.
    Truncated,
}

/// Failure to inspect native task storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionError {
    /// The work limit was zero.
    InvalidLimit,
    /// Submission storage is borrowed by another operation.
    Busy,
    /// Task instrumentation or the visitor panicked.
    Panicked,
}

impl std::fmt::Display for InspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidLimit => "the task inspection limit must be greater than zero",
            Self::Busy => "native task storage is busy",
            Self::Panicked => "the task instrumentation or visitor panicked",
        })
    }
}

impl std::error::Error for InspectionError {}

/// Runtime-owned callbacks used only by an explicitly observed execution cycle.
pub trait PollObserver: std::fmt::Debug {
    /// Receives a description immediately before the task is polled.
    fn poll_started(&self, description: &dyn Any);
    /// Finishes an observed poll, including a task that completed.
    fn poll_finished(&self);
    /// Reports an instrumentation panic without preventing the task from running.
    fn inspection_failed(&self);
}

/// Runtime-owned behavior attached to one task.
pub trait TaskInstrumentation: 'static {
    /// Visits a temporary description of the task.
    fn inspect(&self, _state: TaskState, _visitor: &mut dyn FnMut(&dyn Any)) {}

    /// Runs immediately before the executor polls the task.
    fn poll_start(&mut self) {}

    /// Runs immediately after the executor polls the task.
    fn poll_end(&mut self, _outcome: Poll<()>) {}

    /// Reports successful completion.
    fn completed(&mut self) {}

    /// Reports panic completion.
    fn panicked(&mut self) {}

    /// Reports cancellation.
    fn cancelled(&mut self) {}
}

impl TaskInstrumentation for () {}

pub(crate) struct InspectWith<I, J, D> {
    instrumentation: I,
    inspect: J,
    marker: PhantomData<fn() -> D>,
}

impl<I, J, D> InspectWith<I, J, D> {
    pub(crate) const fn new(instrumentation: I, inspect: J) -> Self {
        Self {
            instrumentation,
            inspect,
            marker: PhantomData,
        }
    }
}

impl<I, J, D> TaskInstrumentation for InspectWith<I, J, D>
where
    I: TaskInstrumentation,
    J: Fn(TaskState) -> Option<D> + 'static,
    D: 'static,
{
    fn inspect(&self, state: TaskState, visitor: &mut dyn FnMut(&dyn Any)) {
        if let Some(description) = (self.inspect)(state) {
            visitor(&description);
        }
    }

    fn poll_start(&mut self) {
        self.instrumentation.poll_start();
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.instrumentation.poll_end(outcome);
    }

    fn completed(&mut self) {
        self.instrumentation.completed();
    }

    fn panicked(&mut self) {
        self.instrumentation.panicked();
    }

    fn cancelled(&mut self) {
        self.instrumentation.cancelled();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    struct Recorder(Rc<RefCell<Vec<&'static str>>>);

    impl TaskInstrumentation for Recorder {
        fn poll_start(&mut self) {
            self.0.borrow_mut().push("start");
        }

        fn poll_end(&mut self, _outcome: Poll<()>) {
            self.0.borrow_mut().push("end");
        }

        fn completed(&mut self) {
            self.0.borrow_mut().push("completed");
        }

        fn panicked(&mut self) {
            self.0.borrow_mut().push("panicked");
        }

        fn cancelled(&mut self) {
            self.0.borrow_mut().push("cancelled");
        }
    }

    #[test]
    fn inspect_wrapper_forwards_instrumentation_callbacks() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut instrumentation = InspectWith::new(Recorder(Rc::clone(&calls)), Some);
        let mut description = None;

        instrumentation.inspect(TaskState::Ready, &mut |value| {
            description = value.downcast_ref::<TaskState>().copied();
        });
        instrumentation.poll_start();
        instrumentation.poll_end(Poll::Pending);
        instrumentation.completed();
        instrumentation.panicked();
        instrumentation.cancelled();

        assert_eq!(description, Some(TaskState::Ready));
        assert_eq!(*calls.borrow(), ["start", "end", "completed", "panicked", "cancelled"]);
    }
}
