// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Poll;

use observed::Sink;
use observed::context::Transfer;

use crate::telemetry;

/// Classifies how the submitter selected this executor for a task.
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

    /// The submitter targeted a specific worker.
    SpecificThread,
}

/// Wraps one top-level executor task poll.
pub trait TaskPoll: 'static {
    /// Wraps one top-level task poll.
    ///
    /// Implementations must call `task` exactly once and return its result.
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()>;
}

impl TaskPoll for () {
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        task()
    }
}

/// Lifecycle metadata for a task registered with [`TaskSet`](crate::TaskSet).
///
/// The executor applies the captured observed context during polling, cleanup,
/// and lifecycle events. The placement value classifies how the caller selected
/// the executor. It does not select a worker. The default has no observed context
/// and uses [`TaskPlacement::Any`].
#[derive(Default, Debug)]
pub struct TaskOptions {
    context: Option<Transfer>,
    placement: TaskPlacement,
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

impl TaskOptions {
    /// Creates options with the specified placement classification.
    #[must_use]
    pub const fn new(placement: TaskPlacement) -> Self {
        Self { context: None, placement }
    }

    /// Creates options that apply the observed `context` for the task lifecycle.
    #[must_use]
    pub fn observed(context: Transfer, placement: TaskPlacement) -> Self {
        Self {
            context: Some(context),
            placement,
        }
    }

    pub(crate) fn with_context<T>(&self, f: impl FnOnce() -> T) -> T {
        let Some(context) = &self.context else {
            return f();
        };

        let _guard = context.apply_current_thread();
        f()
    }

    #[inline]
    pub(crate) fn created_reporter(&self, sink: &Sink) -> Option<TaskCreatedReporter> {
        (!sink.is_noop()).then(|| TaskCreatedReporter {
            context: self.context.clone(),
            placement: self.placement,
            sink: sink.clone(),
        })
    }

    pub(crate) fn report_succeeded(&self, sink: &Sink, polls: u64) {
        if !sink.is_noop() {
            self.with_context(|| telemetry::report_succeeded(sink, polls));
        }
    }

    pub(crate) fn report_panicked(&self, sink: &Sink, polls: u64) {
        if !sink.is_noop() {
            self.with_context(|| telemetry::report_panicked(sink, polls));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(TaskPlacement: UnwindSafe, RefUnwindSafe);
    // Captured enrichment can contain user-defined interior state. Options do
    // not promise that they can cross a caught-unwind boundary.
    assert_not_impl_any!(TaskOptions: UnwindSafe, RefUnwindSafe);

    #[test]
    fn observed_options_keep_context_and_placement() {
        let options = TaskOptions::observed(Sink::noop().transfer_context(), TaskPlacement::SameThread);

        assert!(options.context.is_some());
        assert_eq!(options.placement, TaskPlacement::SameThread);
    }

    #[test]
    fn new_options_keep_placement_without_context() {
        let options = TaskOptions::new(TaskPlacement::SpecificThread);

        assert!(options.context.is_none());
        assert_eq!(options.placement, TaskPlacement::SpecificThread);
    }
}
