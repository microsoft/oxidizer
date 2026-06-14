// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{AbandonedPolicy, ExecutionResult};
use crate::breaker::constants::MIN_SAMPLING_DURATION;

const WINDOW_COUNT: u32 = 10;

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum HealthStatus {
    Healthy,
    Unhealthy,
}

/// Raw tally of execution results over a sampling period.
///
/// Groups the three outcome counters that are tracked and aggregated together throughout the
/// circuit breaker health machinery, so they can be passed around as a single value instead of
/// three loose `u32` arguments.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionInfo {
    pub(crate) success: u32,
    pub(crate) failed: u32,
    pub(crate) abandoned: u32,
}

impl ExecutionInfo {
    #[cfg(test)]
    pub(crate) fn new(success: u32, failed: u32, abandoned: u32) -> Self {
        Self {
            success,
            failed,
            abandoned,
        }
    }

    /// Total number of recorded executions, including abandoned ones.
    pub(crate) fn total(self) -> u32 {
        self.success.saturating_add(self.failed).saturating_add(self.abandoned)
    }

    pub(crate) fn record(&mut self, result: ExecutionResult) {
        match result {
            ExecutionResult::Success => self.success = self.success.saturating_add(1),
            ExecutionResult::Failure => self.failed = self.failed.saturating_add(1),
            ExecutionResult::Abandoned => self.abandoned = self.abandoned.saturating_add(1),
        }
    }

    fn merge(&mut self, other: Self) {
        self.success = self.success.saturating_add(other.success);
        self.failed = self.failed.saturating_add(other.failed);
        self.abandoned = self.abandoned.saturating_add(other.abandoned);
    }
}

/// Aggregated health information that can be used to determine the failure rate and throughput
/// of a service over a recent sampling period.
#[must_use]
#[derive(Debug, Copy, Clone)]
pub(crate) struct HealthInfo {
    pub(crate) counts: ExecutionInfo,
    pub(crate) failure_rate: f32,
    pub(crate) status: HealthStatus,
}

impl HealthInfo {
    pub(crate) fn new(counts: ExecutionInfo, failure_threshold: f32, min_throughput: u32, abandoned_policy: &AbandonedPolicy) -> Self {
        // Abandoned executions (entered but never exited, e.g. a dropped/cancelled future) are always
        // counted towards the reported throughput. How they influence the open/close decision is
        // delegated to the configured [`AbandonedPolicy`], which yields the failure count and the
        // total count the failure rate and minimum-throughput check are evaluated against. That
        // decision total may exclude abandoned executions even though they are reported in the
        // throughput.
        let (decision_failures, decision_total) = abandoned_policy.decision(counts);

        if decision_total == 0 {
            return Self {
                counts,
                failure_rate: 0.0,
                status: HealthStatus::Healthy,
            };
        }

        #[expect(clippy::cast_possible_truncation, reason = "Acceptable")]
        let failure_rate = (f64::from(decision_failures) / f64::from(decision_total)) as f32;

        Self {
            counts,
            failure_rate,
            status: if failure_rate >= failure_threshold && decision_total >= min_throughput {
                HealthStatus::Unhealthy
            } else {
                HealthStatus::Healthy
            },
        }
    }
}

/// Pre-configured builder that creates `HealthMetrics` instances with consistent settings.
#[derive(Debug, Clone)]
pub(crate) struct HealthMetricsBuilder {
    pub(crate) sampling_duration: Duration,
    pub(crate) failure_threshold: f32,
    pub(crate) min_throughput: u32,
    pub(crate) abandoned_policy: AbandonedPolicy,
}

impl HealthMetricsBuilder {
    pub(crate) fn new(sampling_duration: Duration, failure_threshold: f32, min_throughput: u32, abandoned_policy: AbandonedPolicy) -> Self {
        Self {
            sampling_duration: sampling_duration.max(MIN_SAMPLING_DURATION),
            failure_threshold,
            min_throughput,
            abandoned_policy,
        }
    }

    pub(crate) fn build(&self) -> HealthMetrics {
        HealthMetrics::with_policy(
            self.sampling_duration,
            self.failure_threshold,
            self.min_throughput,
            self.abandoned_policy.clone(),
        )
    }
}

/// Tracks execution results over a sliding time window to provide health metrics.
#[derive(Debug)]
pub(crate) struct HealthMetrics {
    sampling_duration: Duration,
    window_duration: Duration,
    windows: VecDeque<Window>,
    failure_threshold: f32,
    min_throughput: u32,
    abandoned_policy: AbandonedPolicy,
}

impl HealthMetrics {
    #[cfg(test)]
    fn new(sampling_duration: Duration, failure_threshold: f32, min_throughput: u32) -> Self {
        Self::with_policy(sampling_duration, failure_threshold, min_throughput, AbandonedPolicy::default())
    }

    fn with_policy(sampling_duration: Duration, failure_threshold: f32, min_throughput: u32, abandoned_policy: AbandonedPolicy) -> Self {
        Self {
            sampling_duration,
            window_duration: sampling_duration / WINDOW_COUNT,
            windows: VecDeque::with_capacity(WINDOW_COUNT as usize),
            failure_threshold,
            min_throughput,
            abandoned_policy,
        }
    }

    pub(crate) fn record(&mut self, result: ExecutionResult, now: Instant) {
        self.current_window(now).update(result);
    }

    /// Returns a mutable reference to the current window, evicting expired windows and creating a
    /// new window when the most recent one is older than the per-window duration.
    fn current_window(&mut self, now: Instant) -> &mut Window {
        // Remove old windows
        while self
            .windows
            .pop_front_if(|front| now.duration_since(front.started_at) > self.sampling_duration)
            .is_some()
        {}

        let needs_new_window = self
            .windows
            .back()
            .is_none_or(|back| now.duration_since(back.started_at) >= self.window_duration);

        if needs_new_window {
            self.windows.push_back(Window::new(now));
        }

        self.windows.back_mut().expect("a current window was just ensured to exist above")
    }

    pub(crate) fn health_info(&self) -> HealthInfo {
        let mut counts = ExecutionInfo::default();
        for w in &self.windows {
            counts.merge(w.counts);
        }

        HealthInfo::new(counts, self.failure_threshold, self.min_throughput, &self.abandoned_policy)
    }
}

#[derive(Debug)]
struct Window {
    counts: ExecutionInfo,
    started_at: Instant,
}

impl Window {
    fn new(started_at: Instant) -> Self {
        Self {
            counts: ExecutionInfo::default(),
            started_at,
        }
    }

    fn update(&mut self, result: ExecutionResult) {
        self.counts.record(result);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[expect(clippy::float_cmp, reason = "simpler tests")]
mod tests {
    use super::*;

    #[test]
    fn factory_ok() {
        let builder = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default());
        let metrics = builder.build();

        assert_eq!(metrics.sampling_duration, Duration::from_secs(10));
        assert_eq!(metrics.window_duration, Duration::from_secs(1));
        assert_eq!(metrics.failure_threshold, 0.5);
        assert_eq!(metrics.min_throughput, 5);

        // small sampling duration is clamped
        let builder = HealthMetricsBuilder::new(Duration::from_millis(500), 0.5, 5, AbandonedPolicy::default());
        let metrics = builder.build();
        assert_eq!(metrics.sampling_duration, MIN_SAMPLING_DURATION);
    }

    #[test]
    fn record_when_empty() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 1);
        assert_eq!(info.failure_rate, 0.0);
    }

    #[test]
    fn create_health_info_healthy_when_not_throughput() {
        let metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);

        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 0);
        assert_eq!(info.failure_rate, 0.0);
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn record_twice() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 2);
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        metrics.record(ExecutionResult::Failure, start);
        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 2);
        assert_eq!(info.failure_rate, 0.5);
        assert_eq!(info.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn record_abandoned_opens_only_when_all_executions_abandoned() {
        let start = Instant::now();

        // No conclusive results recorded: abandoned executions are considered and can make the
        // circuit unhealthy.
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 2);
        metrics.record(ExecutionResult::Abandoned, start);
        metrics.record(ExecutionResult::Abandoned, start);
        let info = metrics.health_info();
        assert_eq!(info.counts.total(), 2);
        assert_eq!(info.failure_rate, 1.0);
        assert_eq!(info.status, HealthStatus::Unhealthy);

        // With at least one success, abandoned executions are ignored.
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 2);
        metrics.record(ExecutionResult::Success, start);
        metrics.record(ExecutionResult::Abandoned, start);
        metrics.record(ExecutionResult::Abandoned, start);
        let info = metrics.health_info();
        assert_eq!(info.counts.total(), 3);
        assert_eq!(info.counts.abandoned, 2);
        assert_eq!(info.failure_rate, 0.0);
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn record_ensure_old_window_discarded() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);

        // Advance time beyond the sampling duration
        let later = start + Duration::from_secs(11);
        metrics.record(ExecutionResult::Success, later);
        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 1);
        assert_eq!(info.failure_rate, 0.0);
    }

    #[test]
    fn new_ensure_initialized_properly() {
        let metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);
        assert_eq!(metrics.sampling_duration, Duration::from_secs(10));
        assert_eq!(metrics.window_duration, Duration::from_secs(1));
        assert!(metrics.windows.is_empty());
    }

    #[test]
    fn ensure_multiple_windows_created() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);
        let start = Instant::now();
        for i in 0..30 {
            let now = start + Duration::from_millis(i * 100);
            metrics.record(ExecutionResult::Success, now);
        }

        assert_eq!(metrics.windows.len(), 3);

        let first_window = &metrics.windows[0];
        assert_eq!(first_window.counts.success, 10);
        assert_eq!(first_window.counts.failed, 0);
        assert_eq!(first_window.started_at, start);

        // discard the first window
        let later = start + Duration::from_secs(12);
        metrics.record(ExecutionResult::Success, later);
        let info = metrics.health_info();

        assert_eq!(metrics.windows.len(), 2);
        assert_eq!(info.counts.total(), 11);
        assert_eq!(info.failure_rate, 0.0);
    }

    mod health_info_create_tests {
        use super::*;

        #[test]
        fn zero_throughput_is_healthy() {
            let info = HealthInfo::new(ExecutionInfo::new(0, 0, 0), 0.5, 10, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(
                (info.counts.total(), info.failure_rate, info.status),
                (0, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn only_successes_is_healthy() {
            let info = HealthInfo::new(ExecutionInfo::new(10, 0, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(
                (info.counts.total(), info.failure_rate, info.status),
                (10, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn only_failures_above_threshold_is_unhealthy() {
            let info = HealthInfo::new(ExecutionInfo::new(0, 10, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(
                (info.counts.total(), info.failure_rate, info.status),
                (10, 1.0, HealthStatus::Unhealthy)
            );
        }

        #[test]
        fn failure_threshold_boundaries() {
            // At threshold
            let info = HealthInfo::new(ExecutionInfo::new(5, 5, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(info.status, HealthStatus::Unhealthy);

            // Below threshold
            let info = HealthInfo::new(ExecutionInfo::new(6, 4, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(info.status, HealthStatus::Healthy);
        }

        #[test]
        fn min_throughput_boundaries() {
            // Below min throughput - healthy despite high failure rate
            let info = HealthInfo::new(ExecutionInfo::new(0, 3, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(info.status, HealthStatus::Healthy);

            // At min throughput - unhealthy with high failure rate
            let info = HealthInfo::new(ExecutionInfo::new(1, 4, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(info.status, HealthStatus::Unhealthy);
        }

        #[test]
        fn edge_cases() {
            // Saturating add
            let info = HealthInfo::new(ExecutionInfo::new(u32::MAX, 1, 0), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(info.counts.total(), u32::MAX);

            // Zero threshold
            let info = HealthInfo::new(ExecutionInfo::new(1, 1, 0), 0.0, 0, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(info.status, HealthStatus::Unhealthy);
        }

        #[test]
        fn abandoned_considered_only_when_all_executions_abandoned() {
            // No conclusive results at all: abandoned executions count as failures and can open the
            // circuit. This is the single degenerate case where abandonment drives the decision.
            let info = HealthInfo::new(ExecutionInfo::new(0, 0, 5), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(
                (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
                (5, 5, 1.0, HealthStatus::Unhealthy)
            );
        }

        #[test]
        fn abandoned_ignored_when_there_are_successes() {
            // At least one success: abandoned executions are tracked and counted towards throughput,
            // but do not contribute to the failure rate.
            let info = HealthInfo::new(ExecutionInfo::new(10, 0, 100), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
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
            let info = HealthInfo::new(ExecutionInfo::new(0, 2, 3), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(
                (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
                (5, 3, 1.0, HealthStatus::Healthy)
            );

            // With enough real failures to meet the minimum throughput, the abandoned executions are
            // still ignored but the real failures alone open the circuit.
            let info = HealthInfo::new(ExecutionInfo::new(0, 5, 3), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
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
            let info = HealthInfo::new(ExecutionInfo::new(1, 9, 100), 0.5, 5, &AbandonedPolicy::when_all_abandoned());
            assert_eq!(
                (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
                (110, 100, 0.9, HealthStatus::Unhealthy)
            );
        }

        #[test]
        fn ignore_policy_never_opens_from_abandoned() {
            let policy = AbandonedPolicy::ignore();

            // Every execution abandoned: ignored entirely, so the circuit stays healthy.
            let info = HealthInfo::new(ExecutionInfo::new(0, 0, 100), 0.5, 5, &policy);
            assert_eq!(
                (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
                (100, 100, 0.0, HealthStatus::Healthy)
            );

            // Abandoned executions are excluded from the decision denominator entirely.
            let info = HealthInfo::new(ExecutionInfo::new(2, 2, 100), 0.5, 5, &policy);
            assert_eq!(
                (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
                (104, 100, 0.5, HealthStatus::Healthy)
            );
        }

        #[test]
        fn as_failures_policy_counts_abandoned_as_failures() {
            let policy = AbandonedPolicy::as_failures();

            // Abandoned executions count towards both the numerator and the denominator.
            let info = HealthInfo::new(ExecutionInfo::new(2, 0, 8), 0.5, 5, &policy);
            assert_eq!(
                (info.counts.total(), info.counts.abandoned, info.failure_rate, info.status),
                (10, 8, 0.8, HealthStatus::Unhealthy)
            );
        }
    }
}
