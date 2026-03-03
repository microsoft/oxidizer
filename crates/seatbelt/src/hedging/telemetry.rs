// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use super::Attempt;
use crate::RecoveryKind;

/// The name of the hedging event for telemetry reporting.
#[cfg(any(feature = "metrics", test))]
pub(super) const HEDGING_EVENT: &str = "hedging";

/// A guard that emits hedging telemetry when dropped.
///
/// - Abandoned futures (dropped before completing): reports with recovery kind `"abandoned"`
/// - Recoverable results: reports with the actual [`RecoveryKind`]
/// - Non-recoverable (accepted) results: disarmed, no telemetry emitted
#[cfg_attr(
    not(any(feature = "logs", feature = "metrics", test)),
    expect(dead_code, reason = "fields are used for telemetry when feature flags are enabled")
)]
pub(super) struct TelemetryGuard {
    pub(super) attempt: Attempt,
    pub(super) hedging_delay: Duration,
    /// `None` means abandoned (future was dropped before completing).
    recovery_kind: Option<RecoveryKind>,
    pub(super) armed: bool,
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(super) telemetry: crate::utils::TelemetryHelper,
}

impl TelemetryGuard {
    /// Creates a new armed guard that defaults to "abandoned" if dropped without
    /// calling [`set_recovery_kind`][Self::set_recovery_kind].
    pub(super) fn new(
        attempt: Attempt,
        hedging_delay: Duration,
        #[cfg(any(feature = "logs", feature = "metrics", test))] telemetry: crate::utils::TelemetryHelper,
    ) -> Self {
        Self {
            attempt,
            hedging_delay,
            recovery_kind: None,
            armed: true,
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry,
        }
    }

    /// Sets the recovery kind for a recoverable result.
    pub(super) fn set_recovery_kind(&mut self, kind: RecoveryKind) {
        self.recovery_kind = Some(kind);
    }

    /// Disarms the guard so no telemetry is emitted on drop.
    ///
    /// Used for non-recoverable (accepted) results.
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }

    #[cfg(any(feature = "logs", feature = "metrics", test))]
    fn recovery_kind_str(&self) -> &'static str {
        match self.recovery_kind {
            None => "abandoned",
            Some(RecoveryKind::Retry) => "retry",
            Some(RecoveryKind::Unavailable) => "unavailable",
            // recovery_kind() only passes Retry/Unavailable, but handle
            // future variants gracefully.
            Some(_) => "unknown",
        }
    }

    #[cfg_attr(
        not(any(feature = "logs", feature = "metrics", test)),
        expect(clippy::unused_self, reason = "used when telemetry features are enabled")
    )]
    fn emit(&self) {
        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.hedging",
                tracing::Level::INFO,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
                resilience.attempt.index = self.attempt.index(),
                resilience.attempt.is_last = self.attempt.is_last(),
                resilience.attempt.recovery.kind = self.recovery_kind_str(),
                resilience.hedging.delay = self.hedging_delay.as_secs_f32(),
            );
        }

        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use crate::attempt::{ATTEMPT_INDEX, ATTEMPT_IS_LAST, ATTEMPT_RECOVERY_KIND};
            use crate::utils::{EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, HEDGING_EVENT),
                opentelemetry::KeyValue::new(ATTEMPT_INDEX, i64::from(self.attempt.index())),
                opentelemetry::KeyValue::new(ATTEMPT_IS_LAST, self.attempt.is_last()),
                opentelemetry::KeyValue::new(ATTEMPT_RECOVERY_KIND, self.recovery_kind_str()),
            ]);
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if self.armed {
            self.emit();
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MetricTester;
    use opentelemetry::KeyValue;
    use tick::Clock;

    fn create_guard(attempt: Attempt, hedging_delay: Duration, telemetry: crate::utils::TelemetryHelper) -> TelemetryGuard {
        TelemetryGuard::new(attempt, hedging_delay, telemetry)
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn guard_emits_on_drop_when_armed() {
        let tester = MetricTester::new();
        let context = crate::ResilienceContext::<String, String>::new(Clock::new_frozen())
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());
        let telemetry = context.create_telemetry("test_hedging".into());

        let guard = create_guard(Attempt::new(1, true), Duration::from_millis(200), telemetry);
        drop(guard);

        tester.assert_attributes(
            &[
                KeyValue::new("resilience.pipeline.name", "test_pipeline"),
                KeyValue::new("resilience.strategy.name", "test_hedging"),
                KeyValue::new("resilience.event.name", "hedging"),
                KeyValue::new("resilience.attempt.index", 1i64),
                KeyValue::new("resilience.attempt.is_last", true),
                KeyValue::new("resilience.attempt.recovery.kind", "abandoned"),
            ],
            Some(6),
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn guard_emits_recovery_kind_when_set() {
        let tester = MetricTester::new();
        let context = crate::ResilienceContext::<String, String>::new(Clock::new_frozen())
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());
        let telemetry = context.create_telemetry("test_hedging".into());

        let mut guard = create_guard(Attempt::new(0, false), Duration::ZERO, telemetry);
        guard.set_recovery_kind(RecoveryKind::Retry);
        drop(guard);

        tester.assert_attributes(&[KeyValue::new("resilience.attempt.recovery.kind", "retry")], Some(6));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn guard_does_not_emit_when_disarmed() {
        let tester = MetricTester::new();
        let context = crate::ResilienceContext::<String, String>::new(Clock::new_frozen())
            .name("test_pipeline")
            .use_metrics(tester.meter_provider());
        let telemetry = context.create_telemetry("test_hedging".into());

        let mut guard = create_guard(Attempt::new(0, false), Duration::ZERO, telemetry);
        guard.disarm();
        drop(guard);

        assert!(tester.collect_attributes().is_empty(), "expected no metrics when guard is disarmed");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn guard_emits_log_with_hedging_delay() {
        use crate::testing::LogCapture;
        use tracing_subscriber::util::SubscriberInitExt;

        let log_capture = LogCapture::new();
        let _default = log_capture.subscriber().set_default();

        let context = crate::ResilienceContext::<String, String>::new(Clock::new_frozen())
            .name("log_pipeline")
            .use_logs();
        let telemetry = context.create_telemetry("log_hedging".into());

        let guard = create_guard(Attempt::new(1, false), Duration::from_millis(250), telemetry);
        drop(guard);

        log_capture.assert_contains("resilience.hedging.delay");
        log_capture.assert_contains("resilience.attempt.recovery.kind");
        log_capture.assert_contains("log_pipeline");
        log_capture.assert_contains("log_hedging");
    }
}
