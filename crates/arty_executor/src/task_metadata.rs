// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use observed::Sink;
use observed::context::Transfer;

use crate::{TaskPanicPayload, telemetry};

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

/// Observes one top-level task poll.
///
/// Implementations must not panic. The executor does not catch panics from
/// these methods.
pub trait TaskPoll: 'static {
    /// Runs immediately before the task poll.
    fn begin(&mut self);

    /// Runs immediately after the task poll.
    ///
    /// The caller also runs this method after it contains a panic from the task.
    fn end(&mut self);
}

/// Polls one concrete task future under its runtime integration policy.
#[doc(hidden)]
pub trait TaskPoller<F: Future>: 'static {
    /// Polls the future and returns a contained task panic.
    fn poll(&mut self, future: Pin<&mut F>, cx: &mut Context<'_>) -> Result<Poll<F::Output>, TaskPanicPayload>;
}

impl<F: Future> TaskPoller<F> for () {
    fn poll(&mut self, mut future: Pin<&mut F>, cx: &mut Context<'_>) -> Result<Poll<F::Output>, TaskPanicPayload> {
        catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx)))
    }
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

    use static_assertions::assert_not_impl_any;

    use super::*;

    // Private metadata has no public type through which to check its unwind traits.
    // Captured enrichment can contain user-defined interior state.
    assert_not_impl_any!(TaskMetadata: UnwindSafe, RefUnwindSafe);
}
