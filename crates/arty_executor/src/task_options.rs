// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::Sink;
use observed::context::Transfer;

use crate::telemetry;

/// Describes how the runtime selected the worker for a task.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TaskPlacement {
    /// The runtime may select any worker.
    #[default]
    Any,

    /// The task targets the scheduler's associated worker.
    SameThread,
}

/// Optional context for a task registered with [`TaskSet`](crate::TaskSet).
///
/// The default options do not apply any enrichment. Runtime integrations can capture the current
/// observed context and attach it to a task with [`observed`](Self::observed).
#[derive(Debug, Default)]
pub struct TaskOptions {
    context: Option<Transfer>,
    placement: TaskPlacement,
}

impl TaskOptions {
    /// Creates options for a task with the specified worker-placement route.
    #[must_use]
    pub const fn new(placement: TaskPlacement) -> Self {
        Self { context: None, placement }
    }

    /// Creates options that apply the captured observed `context` while the task is polled and
    /// while its lifecycle events are emitted.
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

    pub(crate) fn report_created(&self, sink: &Sink) {
        self.with_context(|| telemetry::report_created(sink, self.placement));
    }

    pub(crate) fn report_succeeded(&self, sink: &Sink, polls: u64) {
        self.with_context(|| telemetry::report_succeeded(sink, polls));
    }

    pub(crate) fn report_panicked(&self, sink: &Sink, polls: u64) {
        self.with_context(|| telemetry::report_panicked(sink, polls));
    }
}
