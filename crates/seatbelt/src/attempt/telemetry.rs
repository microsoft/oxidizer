// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::RecoveryKind;
use crate::attempt::Attempt;

/// The recovery kind reported when an attempt is dropped without affirmation.
#[cfg(any(feature = "logs", feature = "metrics", test))]
const ABANDONED_RECOVERY_KIND: &str = "abandoned";

/// Tracks telemetry for a single retry attempt and emits metrics/logs on drop.
///
/// The caller must call [`affirm`](AttemptTelemetry::affirm) to record that the attempt
/// produced a value and to set the [`RecoveryKind`]. If the `AttemptTelemetry` is dropped
/// without affirmation (e.g., due to future cancellation), `resilience.attempt.recovery.kind`
/// reports `"abandoned"`.
///
/// Telemetry is emitted on drop when [`set_emit`](AttemptTelemetry::set_emit) has been called,
/// or unconditionally when the attempt is dropped without affirmation (abandoned).
#[must_use]
pub(crate) struct AttemptTelemetry<'a> {
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) telemetry: &'a crate::utils::TelemetryHelper,
    #[cfg(not(any(feature = "logs", feature = "metrics", test)))]
    pub(crate) _marker: std::marker::PhantomData<&'a ()>,
    #[cfg_attr(
        not(any(feature = "logs", feature = "metrics", test)),
        expect(dead_code, reason = "read by Drop impl when telemetry features are enabled")
    )]
    pub(crate) attempt: Attempt,
    #[cfg_attr(
        not(any(feature = "logs", feature = "metrics", test)),
        expect(dead_code, reason = "read by Drop impl when telemetry features are enabled")
    )]
    pub(crate) event_name: &'static str,
    pub(crate) retry_delay: Duration,
    pub(crate) recovery_kind: Option<RecoveryKind>,
    pub(crate) emit: bool,
}

impl AttemptTelemetry<'_> {
    /// Affirms that the attempt produced a value, setting the recovery kind and retry delay
    /// for telemetry reporting.
    pub(crate) fn affirm(&mut self, recovery_kind: RecoveryKind, retry_delay: Duration) {
        self.recovery_kind = Some(recovery_kind);
        self.retry_delay = retry_delay;
    }

    /// Marks this attempt for telemetry emission on drop.
    pub(crate) fn set_emit(&mut self) {
        self.emit = true;
    }
}

#[cfg(any(feature = "logs", feature = "metrics", test))]
impl Drop for AttemptTelemetry<'_> {
    fn drop(&mut self) {
        // Emit when explicitly requested, or when abandoned (no affirmation).
        if !self.emit && self.recovery_kind.is_some() {
            return;
        }

        let recovery_kind_str = match self.recovery_kind {
            Some(kind) => kind.to_string(),
            None => ABANDONED_RECOVERY_KIND.to_string(),
        };

        #[cfg(any(feature = "logs", test))]
        if self.telemetry.logs_enabled {
            tracing::event!(
                name: "seatbelt.retry",
                tracing::Level::WARN,
                pipeline.name = %self.telemetry.pipeline_name,
                strategy.name = %self.telemetry.strategy_name,
                resilience.attempt.index = self.attempt.index(),
                resilience.attempt.is_last = self.attempt.is_last(),
                resilience.retry.delay = self.retry_delay.as_secs_f32(),
                resilience.attempt.recovery.kind = %recovery_kind_str,
            );
        }

        #[cfg(any(feature = "metrics", test))]
        if self.telemetry.metrics_enabled() {
            use crate::utils::{ATTEMPT_INDEX, ATTEMPT_IS_LAST, ATTEMPT_RECOVERY_KIND, EVENT_NAME, PIPELINE_NAME, STRATEGY_NAME};

            self.telemetry.report_metrics(&[
                opentelemetry::KeyValue::new(PIPELINE_NAME, self.telemetry.pipeline_name.clone()),
                opentelemetry::KeyValue::new(STRATEGY_NAME, self.telemetry.strategy_name.clone()),
                opentelemetry::KeyValue::new(EVENT_NAME, self.event_name),
                opentelemetry::KeyValue::new(ATTEMPT_INDEX, i64::from(self.attempt.index())),
                opentelemetry::KeyValue::new(ATTEMPT_IS_LAST, self.attempt.is_last()),
                opentelemetry::KeyValue::new(ATTEMPT_RECOVERY_KIND, recovery_kind_str),
            ]);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(miri))]
#[cfg(test)]
mod tests {
    use opentelemetry::KeyValue;

    use super::*;
    use crate::testing::MetricTester;
    use crate::utils::{ATTEMPT_RECOVERY_KIND, TelemetryHelper};

    fn create_test_telemetry(tester: &MetricTester) -> TelemetryHelper {
        let meter = crate::metrics::create_meter(tester.meter_provider());
        TelemetryHelper {
            pipeline_name: "test_pipeline".into(),
            strategy_name: "test_retry".into(),
            event_reporter: Some(crate::metrics::create_resilience_event_counter(&meter)),
            logs_enabled: false,
        }
    }

    #[test]
    fn abandoned_when_not_affirmed() {
        let tester = MetricTester::new();
        let helper = create_test_telemetry(&tester);

        {
            let _telemetry = AttemptTelemetry {
                telemetry: &helper,
                attempt: Attempt::new(0, false),
                event_name: "retry",
                retry_delay: Duration::ZERO,
                recovery_kind: None,
                emit: false,
            };
        }

        tester.assert_attributes(&[KeyValue::new(ATTEMPT_RECOVERY_KIND, "abandoned")], None);
    }

    #[test]
    fn affirmed_with_emit_reports_recovery_kind() {
        let tester = MetricTester::new();
        let helper = create_test_telemetry(&tester);

        {
            let mut telemetry = AttemptTelemetry {
                telemetry: &helper,
                attempt: Attempt::new(1, true),
                event_name: "retry",
                retry_delay: Duration::ZERO,
                recovery_kind: None,
                emit: false,
            };
            telemetry.affirm(RecoveryKind::Retry, Duration::from_millis(100));
            telemetry.set_emit();
        }

        tester.assert_attributes(&[KeyValue::new(ATTEMPT_RECOVERY_KIND, "retry")], None);
    }

    #[test]
    fn affirmed_without_emit_does_not_report() {
        let tester = MetricTester::new();
        let helper = create_test_telemetry(&tester);

        {
            let mut telemetry = AttemptTelemetry {
                telemetry: &helper,
                attempt: Attempt::new(0, false),
                event_name: "retry",
                retry_delay: Duration::ZERO,
                recovery_kind: None,
                emit: false,
            };
            telemetry.affirm(RecoveryKind::Never, Duration::ZERO);
            // Don't call set_emit — should NOT emit
        }

        let attributes = tester.collect_attributes();
        assert!(attributes.is_empty(), "expected no telemetry, got: {attributes:?}");
    }
}
