// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::classified;
use observed::context::Transfer;
use observed::{Sink, emit, event};

static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");

#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
struct PlacementLabel(&'static str);

/// Optional context for a task registered with [`TaskSet`](crate::TaskSet).
///
/// The default options do not apply any enrichment. Runtime integrations can capture the current
/// observed context and attach it to a task with [`observed`](Self::observed).
#[derive(Debug, Default)]
pub struct TaskOptions {
    context: Option<Transfer>,
}

impl TaskOptions {
    /// Creates options that apply the captured observed `context` while the task is polled and
    /// while its lifecycle events are emitted.
    #[must_use]
    pub fn observed(context: Transfer) -> Self {
        Self { context: Some(context) }
    }

    pub(crate) fn with_context<T>(&self, f: impl FnOnce() -> T) -> T {
        let Some(context) = &self.context else {
            return f();
        };

        let _guard = context.apply_current_thread();
        f()
    }

    pub(crate) fn report_created(&self, sink: &Sink) {
        if self.context.is_none() {
            return;
        }
        self.with_context(|| {
            emit!(
                sink,
                TaskCreated {
                    placement: PlacementLabel("any")
                }
            );
        });
    }

    pub(crate) fn report_succeeded(&self, sink: &Sink, polls: u64) {
        if self.context.is_none() {
            return;
        }
        emit!(sink, TaskSucceeded { polls });
    }

    pub(crate) fn report_panicked(&self, sink: &Sink, polls: u64) {
        if self.context.is_none() {
            return;
        }
        emit!(sink, TaskPanicked { polls });
    }
}

#[event("arty.rt.task.spawned")]
#[counter(name = "arty.rt.task.spawned")]
struct TaskCreated {
    #[dimension(metric = "placement")]
    placement: PlacementLabel,
}

#[event("arty.rt.task.succeeded")]
#[counter(name = "arty.rt.task.succeeded")]
#[histogram(polls, name = "arty.rt.task.succeeded.polls")]
struct TaskSucceeded {
    #[unredacted]
    polls: u64,
}

#[event("arty.rt.task.panicked")]
#[error("task panicked")]
#[counter(name = "arty.rt.task.panicked")]
#[histogram(polls, name = "arty.rt.task.panicked.polls")]
struct TaskPanicked {
    #[unredacted]
    polls: u64,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use observed_testing::{TEST_ID, test_emitter};

    use super::*;

    #[test]
    fn lifecycle_events_include_poll_count() {
        let (sink, processor) = test_emitter(TEST_ID);
        let options = TaskOptions::observed(sink.transfer_context());

        options.report_created(&sink);
        options.report_succeeded(&sink, 3);

        let events = processor.events();
        assert_eq!(events[0].name(), "arty.rt.task.spawned");
        assert_eq!(events[1].name(), "arty.rt.task.succeeded");
        assert_eq!(events[1].field_metrics()[0].field_key(), "polls");
        assert!((events[1].field_metrics()[0].value() - 3.0).abs() < f64::EPSILON);
    }
}
