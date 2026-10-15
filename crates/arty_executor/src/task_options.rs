// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::Sink;
use observed::context::Transfer;

use crate::telemetry;

/// The terminal outcome of an executor task.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    /// The task completed successfully.
    Succeeded,

    /// The task caught and transported a panic.
    Panicked,
}

/// Describes how the submitter selected this executor for a task.
///
/// This value is lifecycle telemetry metadata. It does not change how the executor schedules or
/// polls the task.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskPlacement {
    /// The submitter did not require a specific worker.
    #[default]
    Any,

    /// The submitter targeted its current worker.
    SameThread,
}

/// Lifecycle metadata for a task registered with [`TaskSet`](crate::TaskSet).
///
/// The executor applies captured observed context while polling, cleaning up, and emitting
/// lifecycle events so all task activity keeps the submission dimensions. Placement records how
/// the caller chose the executor; it does not select a worker. The default options add no context
/// and report [`TaskPlacement::Any`].
#[derive(Debug)]
pub struct TaskOptions<R = ()> {
    context: Option<Transfer>,
    placement: TaskPlacement,
    classify: Option<fn(&R) -> TaskOutcome>,
    capture_panics: bool,
}

impl<R> Default for TaskOptions<R> {
    fn default() -> Self {
        Self::new(TaskPlacement::default())
    }
}

pub(crate) struct TaskCreatedReporter {
    context: Option<Transfer>,
    placement: TaskPlacement,
    sink: Sink,
}

impl TaskCreatedReporter {
    pub(crate) fn report(self) {
        if let Some(context) = self.context {
            let _guard = context.apply_current_thread();
            telemetry::report_created(&self.sink, self.placement);
        } else {
            telemetry::report_created(&self.sink, self.placement);
        }
    }
}

impl<R> TaskOptions<R> {
    /// Creates options with the specified placement metadata.
    #[must_use]
    pub const fn new(placement: TaskPlacement) -> Self {
        Self {
            context: None,
            placement,
            classify: None,
            capture_panics: false,
        }
    }

    /// Creates options that apply the captured observed `context` throughout the task lifecycle.
    #[must_use]
    pub fn observed(context: Transfer, placement: TaskPlacement) -> Self {
        Self {
            context: Some(context),
            placement,
            classify: None,
            capture_panics: false,
        }
    }

    /// Creates observed options for a future that transports its own panic outcome.
    #[doc(hidden)]
    #[must_use]
    pub fn observed_with_outcome(context: Transfer, placement: TaskPlacement, classify: fn(&R) -> TaskOutcome) -> Self {
        Self {
            context: Some(context),
            placement,
            classify: Some(classify),
            capture_panics: true,
        }
    }

    pub(crate) fn with_context<T>(&self, f: impl FnOnce() -> T) -> T {
        let Some(context) = &self.context else {
            return f();
        };

        let _guard = context.apply_current_thread();
        f()
    }

    pub(crate) fn created_reporter(&self, sink: Sink) -> TaskCreatedReporter {
        TaskCreatedReporter {
            context: self.context.clone(),
            placement: self.placement,
            sink,
        }
    }

    pub(crate) fn completed_outcome(&self, result: &R) -> TaskOutcome {
        self.classify.map_or(TaskOutcome::Succeeded, |classify| classify(result))
    }

    pub(crate) fn report_outcome(&self, sink: &Sink, outcome: TaskOutcome, polls: u64) {
        self.with_context(|| match outcome {
            TaskOutcome::Succeeded => telemetry::report_succeeded(sink, polls),
            TaskOutcome::Panicked => telemetry::report_panicked(sink, polls),
        });
    }

    pub(crate) fn report_panicked(&self, sink: &Sink, polls: u64) {
        self.with_context(|| telemetry::report_panicked(sink, polls));
    }

    pub(crate) const fn captures_panics(&self) -> bool {
        self.capture_panics
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn observed_options_keep_context_and_placement() {
        let options = TaskOptions::<()>::observed(Sink::noop().transfer_context(), TaskPlacement::SameThread);

        assert!(options.context.is_some());
        assert_eq!(options.placement, TaskPlacement::SameThread);
        assert!(!options.captures_panics());
    }
}
