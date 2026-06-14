// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::abandoned_policy::Mode;
use super::{AbandonedPolicy, ExecutionInfo, HealthInfo, HealthStatus};

/// Centralized health evaluation logic for the circuit breaker.
///
/// This is the single place where a raw [`ExecutionInfo`] tally is turned into a [`HealthInfo`]
/// verdict. It combines the three inputs that drive the open/close decision — the configured
/// failure threshold, the minimum throughput, and the [`AbandonedPolicy`] — so that the
/// policy-specific handling of abandoned executions and the failure-rate / minimum-throughput
/// evaluation are kept together rather than split across [`HealthInfo`] and [`AbandonedPolicy`].
#[derive(Debug, Clone)]
pub(crate) struct HealthEvaluator {
    failure_threshold: f32,
    min_throughput: u32,
    abandoned_policy: AbandonedPolicy,
}

impl HealthEvaluator {
    pub(crate) fn new(failure_threshold: f32, min_throughput: u32, abandoned_policy: AbandonedPolicy) -> Self {
        Self {
            failure_threshold,
            min_throughput,
            abandoned_policy,
        }
    }

    #[cfg(test)]
    pub(crate) fn failure_threshold(&self) -> f32 {
        self.failure_threshold
    }

    #[cfg(test)]
    pub(crate) fn min_throughput(&self) -> u32 {
        self.min_throughput
    }

    /// Evaluates the health verdict for the given execution counts.
    ///
    /// Abandoned executions (entered but never exited, e.g. a dropped/cancelled future) are always
    /// counted towards the reported throughput in [`HealthInfo::counts`]. Whether they additionally
    /// contribute to the failure rate and minimum-throughput check is decided by [`Self::decision`]
    /// according to the configured [`AbandonedPolicy`].
    pub(crate) fn evaluate(&self, counts: ExecutionInfo) -> HealthInfo {
        let (decision_failures, decision_total) = self.decision(counts);

        if decision_total == 0 {
            return HealthInfo {
                counts,
                failure_rate: 0.0,
                status: HealthStatus::Healthy,
            };
        }

        #[expect(clippy::cast_possible_truncation, reason = "Acceptable")]
        let failure_rate = (f64::from(decision_failures) / f64::from(decision_total)) as f32;

        let status = if failure_rate >= self.failure_threshold && decision_total >= self.min_throughput {
            HealthStatus::Unhealthy
        } else {
            HealthStatus::Healthy
        };

        HealthInfo {
            counts,
            failure_rate,
            status,
        }
    }

    /// Derives the `(failures, total)` pair that the failure rate and minimum-throughput check are
    /// evaluated against, folding abandoned executions in according to the configured policy.
    ///
    /// The returned total may deliberately differ from [`ExecutionInfo::total`], which always
    /// includes abandoned executions for the reported throughput.
    fn decision(&self, counts: ExecutionInfo) -> (u32, u32) {
        match self.abandoned_policy.mode() {
            Mode::Ignore => (counts.failed, counts.succeeded.saturating_add(counts.failed)),
            Mode::AsFailures => (counts.failed.saturating_add(counts.abandoned), counts.total()),
            Mode::AbandonRateThreshold(threshold) => Self::rate_threshold_decision(counts, threshold),
        }
    }

    /// Decision logic for the abandon-rate based rule.
    ///
    /// When the abandon rate (`abandoned / total`) reaches `threshold`, abandoned executions are
    /// counted as failures over the full throughput; otherwise they are excluded from the decision
    /// entirely and only successes and failures are considered.
    fn rate_threshold_decision(counts: ExecutionInfo, threshold: f32) -> (u32, u32) {
        let total = counts.total();
        if total == 0 {
            return (0, 0);
        }

        let abandon_rate = f64::from(counts.abandoned) / f64::from(total);
        if abandon_rate >= f64::from(threshold) {
            (counts.failed.saturating_add(counts.abandoned), total)
        } else {
            (counts.failed, counts.succeeded.saturating_add(counts.failed))
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(counts: ExecutionInfo, failure_threshold: f32, min_throughput: u32, policy: AbandonedPolicy) -> HealthInfo {
        HealthEvaluator::new(failure_threshold, min_throughput, policy).evaluate(counts)
    }

    fn decision(counts: ExecutionInfo, policy: AbandonedPolicy) -> (u32, u32) {
        HealthEvaluator::new(0.5, 5, policy).decision(counts)
    }

    #[test]
    fn zero_throughput_is_healthy() {
        let info = evaluate(ExecutionInfo::new(0, 0, 0), 0.5, 10, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.failure_rate, info.status),
            (0, 0.0, HealthStatus::Healthy)
        );
    }

    #[test]
    fn only_successes_is_healthy() {
        let info = evaluate(ExecutionInfo::new(10, 0, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.failure_rate, info.status),
            (10, 0.0, HealthStatus::Healthy)
        );
    }

    #[test]
    fn only_failures_above_threshold_is_unhealthy() {
        let info = evaluate(ExecutionInfo::new(0, 10, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.failure_rate, info.status),
            (10, 1.0, HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn failure_threshold_boundaries() {
        // At threshold
        let info = evaluate(ExecutionInfo::new(5, 5, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(info.status, HealthStatus::Unhealthy);

        // Below threshold
        let info = evaluate(ExecutionInfo::new(6, 4, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn min_throughput_boundaries() {
        // Below min throughput - healthy despite high failure rate
        let info = evaluate(ExecutionInfo::new(0, 3, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(info.status, HealthStatus::Healthy);

        // At min throughput - unhealthy with high failure rate
        let info = evaluate(ExecutionInfo::new(1, 4, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(info.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn edge_cases() {
        // Saturating add
        let info = evaluate(ExecutionInfo::new(u32::MAX, 1, 0), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(info.counts.total(), u32::MAX);

        // Zero threshold
        let info = evaluate(ExecutionInfo::new(1, 1, 0), 0.0, 0, AbandonedPolicy::when_all_abandoned());
        assert_eq!(info.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn abandoned_considered_only_when_all_executions_abandoned() {
        // No conclusive results at all: abandoned executions count as failures and can open the
        // circuit. This is the single degenerate case where abandonment drives the decision.
        let info = evaluate(ExecutionInfo::new(0, 0, 5), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (5, 5, 1.0, HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn abandoned_ignored_when_there_are_successes() {
        // At least one success: abandoned executions are tracked and counted towards throughput,
        // but do not contribute to the failure rate.
        let info = evaluate(ExecutionInfo::new(10, 0, 100), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (110, 100, 0.0, HealthStatus::Healthy)
        );
    }

    #[test]
    fn abandoned_ignored_when_failures_present() {
        // Once there is at least one conclusive result (here a failure), abandoned executions are
        // ignored entirely and the decision is made purely on successes/failures. The two real
        // failures are below the minimum throughput, so the circuit stays healthy even though the
        // abandoned executions would otherwise have pushed the total over the threshold.
        let info = evaluate(ExecutionInfo::new(0, 2, 3), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (5, 3, 1.0, HealthStatus::Healthy)
        );

        // With enough real failures to meet the minimum throughput, the abandoned executions are
        // still ignored but the real failures alone open the circuit.
        let info = evaluate(ExecutionInfo::new(0, 5, 3), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (8, 3, 1.0, HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn abandoned_does_not_dilute_real_failures() {
        // A flood of abandoned executions must not mask a genuine burst of failures: with
        // successes present the decision is made on successes/failures only, excluding the
        // abandoned executions from the denominator.
        let info = evaluate(ExecutionInfo::new(1, 9, 100), 0.5, 5, AbandonedPolicy::when_all_abandoned());
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (110, 100, 0.9, HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn ignore_policy_never_opens_from_abandoned() {
        let policy = AbandonedPolicy::ignore();

        // Every execution abandoned: ignored entirely, so the circuit stays healthy.
        let info = evaluate(ExecutionInfo::new(0, 0, 100), 0.5, 5, policy.clone());
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (100, 100, 0.0, HealthStatus::Healthy)
        );

        // Abandoned executions are excluded from the decision denominator entirely.
        let info = evaluate(ExecutionInfo::new(2, 2, 100), 0.5, 5, policy);
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (104, 100, 0.5, HealthStatus::Healthy)
        );
    }

    #[test]
    fn as_failures_policy_counts_abandoned_as_failures() {
        let policy = AbandonedPolicy::as_failures();

        // Abandoned executions count towards both the numerator and the denominator.
        let info = evaluate(ExecutionInfo::new(2, 0, 8), 0.5, 5, policy);
        assert_eq!(
            (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
            (10, 8, 0.8, HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn decision_ignore_excludes_abandoned() {
        let policy = AbandonedPolicy::ignore();
        assert_eq!(decision(ExecutionInfo::new(5, 1, 10), policy.clone()), (1, 6));
        // Every execution abandoned: nothing conclusive, so the decision total is zero.
        assert_eq!(decision(ExecutionInfo::new(0, 0, 10), policy), (0, 0));
    }

    #[test]
    fn decision_when_all_abandoned_considers_abandoned_only_when_all_abandoned() {
        let policy = AbandonedPolicy::when_all_abandoned();
        assert_eq!(decision(ExecutionInfo::new(0, 0, 10), policy.clone()), (10, 10));
        assert_eq!(decision(ExecutionInfo::new(1, 0, 10), policy.clone()), (0, 1));
        assert_eq!(decision(ExecutionInfo::new(0, 2, 10), policy), (2, 2));
    }

    #[test]
    fn decision_rate_threshold_counts_abandoned_when_rate_reached() {
        let policy = AbandonedPolicy::abandon_rate_threshold(0.5);
        // 70% abandoned: at or above the threshold, abandoned count as failures over the full total.
        assert_eq!(decision(ExecutionInfo::new(2, 1, 7), policy.clone()), (8, 10));
        // Exactly at the threshold (50% abandoned): abandoned still count as failures.
        assert_eq!(decision(ExecutionInfo::new(3, 2, 5), policy), (7, 10));
    }

    #[test]
    fn decision_rate_threshold_ignores_abandoned_below_rate() {
        let policy = AbandonedPolicy::abandon_rate_threshold(0.5);
        // 10% abandoned: below the threshold, abandoned are excluded from the decision entirely.
        assert_eq!(decision(ExecutionInfo::new(5, 4, 1), policy.clone()), (4, 9));
        // No executions at all: nothing conclusive.
        assert_eq!(decision(ExecutionInfo::new(0, 0, 0), policy), (0, 0));
    }

    #[test]
    fn decision_as_failures_always_counts_abandoned() {
        let policy = AbandonedPolicy::as_failures();
        assert_eq!(decision(ExecutionInfo::new(5, 1, 10), policy.clone()), (11, 16));
        assert_eq!(decision(ExecutionInfo::new(0, 0, 10), policy), (10, 10));
    }
}
