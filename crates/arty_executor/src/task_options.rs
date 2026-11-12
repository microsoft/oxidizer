// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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

/// Lifecycle metadata for a task registered with [`TaskSet`](crate::TaskSet).
///
/// The executor applies the captured observed context during polling, cleanup,
/// and lifecycle events. The placement value classifies how the caller selected
/// the executor. It does not select a worker. The default has no observed context
/// and uses [`TaskPlacement::Any`].
#[derive(Debug, Default)]
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
