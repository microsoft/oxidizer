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

#[derive(Debug, Default)]
pub(crate) struct TaskMetadata {
    pub(crate) context: Option<Transfer>,
    pub(crate) placement: TaskPlacement,
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

impl TaskMetadata {
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
    // Captured enrichment can contain user-defined interior state. Metadata does
    // not promise that it can cross a caught-unwind boundary.
    assert_not_impl_any!(TaskMetadata: UnwindSafe, RefUnwindSafe);
}
