// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::ExecutionResult;
use crate::breaker::constants::MIN_SAMPLING_DURATION;

const WINDOW_COUNT: u32 = 10;

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum HealthStatus {
    Healthy,
    Unhealthy,
}

/// Aggregated health information that can be used to determine the failure rate and throughput
/// of a service over a recent sampling period.
#[must_use]
#[derive(Debug, Copy, Clone)]
pub(crate) struct HealthInfo {
    throughput: u32,
    abandoned: u32,
    failure_rate: f32,
    health_status: HealthStatus,
}

impl HealthInfo {
    pub(crate) fn new(successes: u32, failures: u32, abandoned: u32, failure_threshold: f32, min_throughput: u32) -> Self {
        // Abandoned executions (entered but never exited, e.g. a dropped/cancelled future) are
        // always counted towards throughput, but only contribute to the failure rate when there
        // were no successful executions during the sampling period. This handles the pathological
        // case where every execution is abandoned: without this, the circuit would never observe
        // any result and could never open.
        let throughput = successes.saturating_add(failures).saturating_add(abandoned);

        if throughput == 0 {
            return Self {
                throughput: 0,
                abandoned: 0,
                failure_rate: 0.0,
                health_status: HealthStatus::Healthy,
            };
        }

        let failures = if successes == 0 {
            failures.saturating_add(abandoned)
        } else {
            failures
        };

        #[expect(clippy::cast_possible_truncation, reason = "Acceptable")]
        let failure_rate = (f64::from(failures) / f64::from(throughput)) as f32;

        Self {
            throughput,
            abandoned,
            failure_rate,
            health_status: if failure_rate >= failure_threshold && throughput >= min_throughput {
                HealthStatus::Unhealthy
            } else {
                HealthStatus::Healthy
            },
        }
    }

    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(dead_code, reason = "trying to avoid dead code here leads to too much conditionals")
    )]
    pub(crate) fn abandoned(&self) -> u32 {
        self.abandoned
    }

    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(dead_code, reason = "trying to avoid dead code here leads to too much conditionals")
    )]
    pub(crate) fn throughput(&self) -> u32 {
        self.throughput
    }

    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(dead_code, reason = "trying to avoid dead code here leads to too much conditionals")
    )]
    pub(crate) fn failure_rate(&self) -> f32 {
        self.failure_rate
    }

    pub(crate) fn status(&self) -> HealthStatus {
        self.health_status
    }
}

/// Pre-configured builder that creates `HealthMetrics` instances with consistent settings.
#[derive(Debug, Clone)]
pub(crate) struct HealthMetricsBuilder {
    pub(crate) sampling_duration: Duration,
    pub(crate) failure_threshold: f32,
    pub(crate) min_throughput: u32,
}

impl HealthMetricsBuilder {
    pub(crate) fn new(sampling_duration: Duration, failure_threshold: f32, min_throughput: u32) -> Self {
        Self {
            sampling_duration: sampling_duration.max(MIN_SAMPLING_DURATION),
            failure_threshold,
            min_throughput,
        }
    }

    pub(crate) fn build(&self) -> HealthMetrics {
        HealthMetrics::new(self.sampling_duration, self.failure_threshold, self.min_throughput)
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
}

impl HealthMetrics {
    fn new(sampling_duration: Duration, failure_threshold: f32, min_throughput: u32) -> Self {
        Self {
            sampling_duration,
            window_duration: sampling_duration / WINDOW_COUNT,
            windows: VecDeque::with_capacity(WINDOW_COUNT as usize),
            failure_threshold,
            min_throughput,
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
        let mut successes = 0_u32;
        let mut failures = 0_u32;
        let mut abandoned = 0_u32;

        for w in &self.windows {
            successes = successes.saturating_add(w.successes);
            failures = failures.saturating_add(w.failures);
            abandoned = abandoned.saturating_add(w.abandoned);
        }

        HealthInfo::new(successes, failures, abandoned, self.failure_threshold, self.min_throughput)
    }
}

#[derive(Debug)]
struct Window {
    successes: u32,
    failures: u32,
    abandoned: u32,
    started_at: Instant,
}

impl Window {
    fn new(started_at: Instant) -> Self {
        Self {
            successes: 0,
            failures: 0,
            abandoned: 0,
            started_at,
        }
    }

    fn update(&mut self, result: ExecutionResult) {
        match result {
            ExecutionResult::Success => self.successes = self.successes.saturating_add(1),
            ExecutionResult::Failure => self.failures = self.failures.saturating_add(1),
            ExecutionResult::Abandoned => self.abandoned = self.abandoned.saturating_add(1),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[expect(clippy::float_cmp, reason = "simpler tests")]
mod tests {
    use super::*;

    #[test]
    fn factory_ok() {
        let builder = HealthMetricsBuilder::new(Duration::from_secs(10), 0.5, 5);
        let metrics = builder.build();

        assert_eq!(metrics.sampling_duration, Duration::from_secs(10));
        assert_eq!(metrics.window_duration, Duration::from_secs(1));
        assert_eq!(metrics.failure_threshold, 0.5);
        assert_eq!(metrics.min_throughput, 5);

        // small sampling duration is clamped
        let builder = HealthMetricsBuilder::new(Duration::from_millis(500), 0.5, 5);
        let metrics = builder.build();
        assert_eq!(metrics.sampling_duration, MIN_SAMPLING_DURATION);
    }

    #[test]
    fn record_when_empty() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        let info = metrics.health_info();

        assert_eq!(info.throughput(), 1);
        assert_eq!(info.failure_rate(), 0.0);
    }

    #[test]
    fn create_health_info_healthy_when_not_throughput() {
        let metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);

        let info = metrics.health_info();

        assert_eq!(info.throughput(), 0);
        assert_eq!(info.failure_rate(), 0.0);
        assert_eq!(info.status(), HealthStatus::Healthy);
    }

    #[test]
    fn record_twice() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 2);
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        metrics.record(ExecutionResult::Failure, start);
        let info = metrics.health_info();

        assert_eq!(info.throughput(), 2);
        assert_eq!(info.failure_rate(), 0.5);
        assert_eq!(info.status(), HealthStatus::Unhealthy);
    }

    #[test]
    fn record_abandoned_opens_only_when_no_successes() {
        let start = Instant::now();

        // No successes recorded: abandoned executions are considered and can make the circuit unhealthy.
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 2);
        metrics.record(ExecutionResult::Abandoned, start);
        metrics.record(ExecutionResult::Abandoned, start);
        let info = metrics.health_info();
        assert_eq!(info.throughput(), 2);
        assert_eq!(info.failure_rate(), 1.0);
        assert_eq!(info.status(), HealthStatus::Unhealthy);

        // With at least one success, abandoned executions are ignored.
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 2);
        metrics.record(ExecutionResult::Success, start);
        metrics.record(ExecutionResult::Abandoned, start);
        metrics.record(ExecutionResult::Abandoned, start);
        let info = metrics.health_info();
        assert_eq!(info.throughput(), 3);
        assert_eq!(info.abandoned(), 2);
        assert_eq!(info.failure_rate(), 0.0);
        assert_eq!(info.status(), HealthStatus::Healthy);
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

        assert_eq!(info.throughput(), 1);
        assert_eq!(info.failure_rate(), 0.0);
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
        assert_eq!(first_window.successes, 10);
        assert_eq!(first_window.failures, 0);
        assert_eq!(first_window.started_at, start);

        // discard the first window
        let later = start + Duration::from_secs(12);
        metrics.record(ExecutionResult::Success, later);
        let info = metrics.health_info();

        assert_eq!(metrics.windows.len(), 2);
        assert_eq!(info.throughput(), 11);
        assert_eq!(info.failure_rate(), 0.0);
    }

    mod health_info_create_tests {
        use super::*;

        #[test]
        fn zero_throughput_is_healthy() {
            let info = HealthInfo::new(0, 0, 0, 0.5, 10);
            assert_eq!(
                (info.throughput(), info.failure_rate(), info.status()),
                (0, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn only_successes_is_healthy() {
            let info = HealthInfo::new(10, 0, 0, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.failure_rate(), info.status()),
                (10, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn only_failures_above_threshold_is_unhealthy() {
            let info = HealthInfo::new(0, 10, 0, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.failure_rate(), info.status()),
                (10, 1.0, HealthStatus::Unhealthy)
            );
        }

        #[test]
        fn failure_threshold_boundaries() {
            // At threshold
            let info = HealthInfo::new(5, 5, 0, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Unhealthy);

            // Below threshold
            let info = HealthInfo::new(6, 4, 0, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Healthy);
        }

        #[test]
        fn min_throughput_boundaries() {
            // Below min throughput - healthy despite high failure rate
            let info = HealthInfo::new(0, 3, 0, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Healthy);

            // At min throughput - unhealthy with high failure rate
            let info = HealthInfo::new(1, 4, 0, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Unhealthy);
        }

        #[test]
        fn edge_cases() {
            // Saturating add
            let info = HealthInfo::new(u32::MAX, 1, 0, 0.5, 5);
            assert_eq!(info.throughput(), u32::MAX);

            // Zero threshold
            let info = HealthInfo::new(1, 1, 0, 0.0, 0);
            assert_eq!(info.status(), HealthStatus::Unhealthy);
        }

        #[test]
        fn abandoned_considered_when_no_successes() {
            // No successes: abandoned executions count as failures and can open the circuit.
            let info = HealthInfo::new(0, 0, 5, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.abandoned(), info.failure_rate(), info.status()),
                (5, 5, 1.0, HealthStatus::Unhealthy)
            );
        }

        #[test]
        fn abandoned_ignored_when_there_are_successes() {
            // At least one success: abandoned executions are tracked and counted towards throughput,
            // but do not contribute to the failure rate.
            let info = HealthInfo::new(10, 0, 100, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.abandoned(), info.failure_rate(), info.status()),
                (110, 100, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn abandoned_combined_with_failures_when_no_successes() {
            // No successes: abandoned are added on top of failures.
            let info = HealthInfo::new(0, 2, 3, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.abandoned(), info.failure_rate(), info.status()),
                (5, 3, 1.0, HealthStatus::Unhealthy)
            );
        }
    }
}
