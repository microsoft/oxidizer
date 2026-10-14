// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::classified;
use observed::context::Transfer;
use observed::{Sink, emit, event};

static SYSTEM_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("arty", "SystemMetadata");

#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
struct PlacementLabel(&'static str);

/// The terminal outcome of an observed executor task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TaskOutcome {
    /// The task completed successfully.
    Succeeded,

    /// The task caught and reported a panic.
    Panicked,
}

/// Optional context and lifecycle reporting for a task registered with [`TaskSet`](crate::TaskSet).
///
/// The default options do not emit lifecycle telemetry. Runtime integrations can use
/// [`observed`](Self::observed) to provide the sink, captured enrichment context, placement, and
/// an output classifier that identifies the task's terminal outcome.
#[derive(Debug)]
pub struct TaskOptions<R> {
    telemetry: Option<TaskTelemetry<R>>,
}

impl<R> TaskOptions<R> {
    /// Creates options that emit task lifecycle telemetry through `sink`.
    ///
    /// The captured `context` is applied while the task is polled and while its lifecycle events
    /// are emitted. `classify` returns the terminal outcome to report, or `None` when a completed
    /// future does not represent a terminal task outcome, such as cancellation during shutdown.
    #[must_use]
    pub fn observed(sink: Sink, context: Transfer, placement: &'static str, classify: fn(&R) -> Option<TaskOutcome>) -> Self {
        Self {
            telemetry: Some(TaskTelemetry {
                sink,
                context,
                placement,
                classify,
            }),
        }
    }

    pub(crate) fn with_context<T>(&self, f: impl FnOnce() -> T) -> T {
        let Some(telemetry) = &self.telemetry else {
            return f();
        };

        let _guard = telemetry.context.apply_current_thread();
        f()
    }

    pub(crate) fn report_created(&self) {
        let Some(telemetry) = &self.telemetry else {
            return;
        };

        self.with_context(|| {
            emit!(
                telemetry.sink,
                TaskCreated {
                    placement: PlacementLabel(telemetry.placement)
                }
            );
        });
    }

    pub(crate) fn classify(&self, result: &R) -> Option<TaskOutcome> {
        self.telemetry.as_ref().and_then(|telemetry| (telemetry.classify)(result))
    }

    pub(crate) fn report_terminal(&self, outcome: TaskOutcome, polls: u64) {
        let Some(telemetry) = &self.telemetry else {
            return;
        };

        match outcome {
            TaskOutcome::Succeeded => emit!(telemetry.sink, TaskSucceeded { polls }),
            TaskOutcome::Panicked => emit!(telemetry.sink, TaskPanicked { polls }),
        }
    }
}

impl<R> Default for TaskOptions<R> {
    fn default() -> Self {
        Self { telemetry: None }
    }
}

#[derive(Debug)]
struct TaskTelemetry<R> {
    sink: Sink,
    context: Transfer,
    placement: &'static str,
    classify: fn(&R) -> Option<TaskOutcome>,
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
    use observed::Value;
    use observed_testing::{TEST_ID, test_emitter};

    use super::*;

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        clippy::unnecessary_wraps,
        reason = "TaskOptions classifiers receive outputs by reference and may omit terminal reporting"
    )]
    fn succeeded(_: &()) -> Option<TaskOutcome> {
        Some(TaskOutcome::Succeeded)
    }

    #[test]
    fn lifecycle_events_include_placement_and_poll_count() {
        let (sink, processor) = test_emitter(TEST_ID);
        let options = TaskOptions::observed(sink.clone(), sink.transfer_context(), "any", succeeded);

        options.report_created();
        options.report_terminal(TaskOutcome::Succeeded, 3);

        let events = processor.events();
        assert_eq!(events[0].name(), "arty.rt.task.spawned");
        assert_eq!(
            events[0]
                .dimensions()
                .into_iter()
                .find(|(key, _)| key == "placement")
                .map(|(_, value)| value),
            Some(Value::from("any"))
        );
        assert_eq!(events[1].name(), "arty.rt.task.succeeded");
        assert_eq!(events[1].field_metrics()[0].field_key(), "polls");
        assert!((events[1].field_metrics()[0].value() - 3.0).abs() < f64::EPSILON);
    }
}
