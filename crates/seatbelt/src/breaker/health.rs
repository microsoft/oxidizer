// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{AbandonedPolicy, ExecutionResult, HealthEvaluator};
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
    pub(crate) succeeded: u32,
    pub(crate) failed: u32,
    pub(crate) abandoned: u32,
}

impl ExecutionInfo {
    #[cfg(test)]
    pub(crate) fn new(succeeded: u32, failed: u32, abandoned: u32) -> Self {
        Self {
            succeeded,
            failed,
            abandoned,
        }
    }

    /// Total number of recorded executions, including abandoned ones.
    pub(crate) fn total(self) -> u32 {
        self.succeeded.saturating_add(self.failed).saturating_add(self.abandoned)
    }

    pub(crate) fn record(&mut self, result: ExecutionResult) {
        match result {
            ExecutionResult::Success => self.succeeded = self.succeeded.saturating_add(1),
            ExecutionResult::Failure => self.failed = self.failed.saturating_add(1),
            ExecutionResult::Abandoned => self.abandoned = self.abandoned.saturating_add(1),
        }
    }

    fn merge(&mut self, other: Self) {
        self.succeeded = self.succeeded.saturating_add(other.succeeded);
        self.failed = self.failed.saturating_add(other.failed);
        self.abandoned = self.abandoned.saturating_add(other.abandoned);
    }
}

/// Aggregated health information that can be used to determine the failure rate and throughput
/// of a service over a recent sampling period.
///
/// This is a plain verdict produced by [`HealthEvaluator`]; all of the evaluation logic (how
/// abandoned executions are folded in, and how the failure rate and minimum throughput are turned
/// into a [`HealthStatus`]) lives there.
#[must_use]
#[derive(Debug, Copy, Clone)]
pub(crate) struct HealthInfo {
    pub(crate) counts: ExecutionInfo,
    pub(crate) status: HealthStatus,
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
        HealthMetrics::new(
            self.sampling_duration,
            HealthEvaluator::new(self.failure_threshold, self.min_throughput, self.abandoned_policy.clone()),
        )
    }
}

/// Tracks execution results over a sliding time window to provide health metrics.
#[derive(Debug)]
pub(crate) struct HealthMetrics {
    sampling_duration: Duration,
    window_duration: Duration,
    windows: VecDeque<Window>,
    evaluator: HealthEvaluator,
}

impl HealthMetrics {
    fn new(sampling_duration: Duration, evaluator: HealthEvaluator) -> Self {
        Self {
            sampling_duration,
            window_duration: sampling_duration / WINDOW_COUNT,
            windows: VecDeque::with_capacity(WINDOW_COUNT as usize),
            evaluator,
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

        self.evaluator.evaluate(counts)
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
mod tests {
    use super::*;

    #[test]
    fn factory_ok() {
        let builder = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default());
        let metrics = builder.build();

        assert_eq!(metrics.sampling_duration, Duration::from_secs(10));
        assert_eq!(metrics.window_duration, Duration::from_secs(1));

        // small sampling duration is clamped
        let builder = HealthMetricsBuilder::new(Duration::from_millis(500), 0.5, 5, AbandonedPolicy::default());
        let metrics = builder.build();
        assert_eq!(metrics.sampling_duration, MIN_SAMPLING_DURATION);
    }

    #[test]
    fn record_when_empty() {
        let mut metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default()).build();
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 1);
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn create_health_info_healthy_when_not_throughput() {
        let metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default()).build();

        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 0);
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn record_twice() {
        let mut metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 2, AbandonedPolicy::default()).build();
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        metrics.record(ExecutionResult::Failure, start);
        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 2);
        assert_eq!(info.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn record_abandoned_opens_only_when_all_executions_abandoned() {
        let start = Instant::now();

        // No conclusive results recorded: abandoned executions are considered and can make the
        // circuit unhealthy.
        let mut metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 2, AbandonedPolicy::default()).build();
        metrics.record(ExecutionResult::Abandoned, start);
        metrics.record(ExecutionResult::Abandoned, start);
        let info = metrics.health_info();
        assert_eq!(info.counts.total(), 2);
        assert_eq!(info.status, HealthStatus::Unhealthy);

        // With at least one success, abandoned executions are ignored.
        let mut metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 2, AbandonedPolicy::default()).build();
        metrics.record(ExecutionResult::Success, start);
        metrics.record(ExecutionResult::Abandoned, start);
        metrics.record(ExecutionResult::Abandoned, start);
        let info = metrics.health_info();
        assert_eq!(info.counts.total(), 3);
        assert_eq!(info.counts.abandoned, 2);
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn record_ensure_old_window_discarded() {
        let mut metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default()).build();
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);

        // Advance time beyond the sampling duration
        let later = start + Duration::from_secs(11);
        metrics.record(ExecutionResult::Success, later);
        let info = metrics.health_info();

        assert_eq!(info.counts.total(), 1);
        assert_eq!(info.status, HealthStatus::Healthy);
    }

    #[test]
    fn new_ensure_initialized_properly() {
        let metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default()).build();
        assert_eq!(metrics.sampling_duration, Duration::from_secs(10));
        assert_eq!(metrics.window_duration, Duration::from_secs(1));
        assert!(metrics.windows.is_empty());
    }

    #[test]
    fn ensure_multiple_windows_created() {
        let mut metrics = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5, AbandonedPolicy::default()).build();
        let start = Instant::now();
        for i in 0..30 {
            let now = start + Duration::from_millis(i * 100);
            metrics.record(ExecutionResult::Success, now);
        }

        assert_eq!(metrics.windows.len(), 3);

        let first_window = &metrics.windows[0];
        assert_eq!(first_window.counts.succeeded, 10);
        assert_eq!(first_window.counts.failed, 0);
        assert_eq!(first_window.started_at, start);

        // discard the first window
        let later = start + Duration::from_secs(12);
        metrics.record(ExecutionResult::Success, later);
        let info = metrics.health_info();

        assert_eq!(metrics.windows.len(), 2);
        assert_eq!(info.counts.total(), 11);
        assert_eq!(info.status, HealthStatus::Healthy);
    }
}
