// Copyright (c) Microsoft Corporation.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::ExecutionResult;
use crate::circuit_breaker::constants::MIN_SAMPLING_DURATION;

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
    failure_rate: f32,
    health_status: HealthStatus,
}

impl HealthInfo {
    pub fn new(successes: u32, failures: u32, failure_threshold: f32, min_throughput: u32) -> Self {
        let throughput = successes.saturating_add(failures);

        if throughput == 0 {
            return Self {
                throughput: 0,
                failure_rate: 0.0,
                health_status: HealthStatus::Healthy,
            };
        }

        #[expect(clippy::cast_possible_truncation, reason = "Acceptable")]
        let failure_rate = (f64::from(failures) / f64::from(throughput)) as f32;

        Self {
            throughput,
            failure_rate,
            health_status: if failure_rate >= failure_threshold && throughput >= min_throughput {
                HealthStatus::Unhealthy
            } else {
                HealthStatus::Healthy
            },
        }
    }

    pub fn throughput(&self) -> u32 {
        self.throughput
    }

    pub fn failure_rate(&self) -> f32 {
        self.failure_rate
    }

    pub fn status(&self) -> HealthStatus {
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
    pub fn new(sampling_duration: Duration, failure_threshold: f32, min_throughput: u32) -> Self {
        Self {
            sampling_duration: sampling_duration.max(MIN_SAMPLING_DURATION),
            failure_threshold,
            min_throughput,
        }
    }

    pub fn build(&self) -> HealthMetrics {
        HealthMetrics::new(
            self.sampling_duration,
            self.failure_threshold,
            self.min_throughput,
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

    pub fn record(&mut self, result: ExecutionResult, now: Instant) {
        // Remove old windows
        while let Some(front) = self.windows.front()
            && now.duration_since(front.started_at) > self.sampling_duration
        {
            self.windows.pop_front();
        }

        // Get or create the current window
        if let Some(back) = self.windows.back_mut()
            && now.duration_since(back.started_at) < self.window_duration
        {
            // Update the existing window
            back.update(result);
        } else {
            // Create a new window
            let mut new_window = Window::new(now);
            new_window.update(result);
            self.windows.push_back(new_window);
        }
    }

    pub fn health_info(&self) -> HealthInfo {
        let mut successes = 0_u32;
        let mut failures = 0_u32;

        for w in &self.windows {
            successes = successes.saturating_add(w.successes);
            failures = failures.saturating_add(w.failures);
        }

        HealthInfo::new(
            successes,
            failures,
            self.failure_threshold,
            self.min_throughput,
        )
    }
}

#[derive(Debug)]
struct Window {
    successes: u32,
    failures: u32,
    started_at: Instant,
}

impl Window {
    fn new(started_at: Instant) -> Self {
        Self {
            successes: 0,
            failures: 0,
            started_at,
        }
    }

    fn update(&mut self, result: ExecutionResult) {
        match result {
            ExecutionResult::Success => self.successes += 1,
            ExecutionResult::Failure => self.failures += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
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
    #[expect(clippy::float_cmp, reason = "Test")]
    fn record_when_empty() {
        let mut metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);
        let start = Instant::now();
        metrics.record(ExecutionResult::Success, start);
        let info = metrics.health_info();

        assert_eq!(info.throughput(), 1);
        assert_eq!(info.failure_rate(), 0.0);
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
    fn create_health_info_healthy_when_not_throughput() {
        let metrics = HealthMetrics::new(Duration::from_secs(10), 0.5, 5);

        let info = metrics.health_info();

        assert_eq!(info.throughput(), 0);
        assert_eq!(info.failure_rate(), 0.0);
        assert_eq!(info.status(), HealthStatus::Healthy);
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
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
    #[expect(clippy::float_cmp, reason = "Test")]
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
    #[expect(clippy::float_cmp, reason = "Test")]
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
            let info = HealthInfo::new(0, 0, 0.5, 10);
            assert_eq!(
                (info.throughput(), info.failure_rate(), info.status()),
                (0, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn only_successes_is_healthy() {
            let info = HealthInfo::new(10, 0, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.failure_rate(), info.status()),
                (10, 0.0, HealthStatus::Healthy)
            );
        }

        #[test]
        fn only_failures_above_threshold_is_unhealthy() {
            let info = HealthInfo::new(0, 10, 0.5, 5);
            assert_eq!(
                (info.throughput(), info.failure_rate(), info.status()),
                (10, 1.0, HealthStatus::Unhealthy)
            );
        }

        #[test]
        fn failure_threshold_boundaries() {
            // At threshold
            let info = HealthInfo::new(5, 5, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Unhealthy);

            // Below threshold
            let info = HealthInfo::new(6, 4, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Healthy);
        }

        #[test]
        fn min_throughput_boundaries() {
            // Below min throughput - healthy despite high failure rate
            let info = HealthInfo::new(0, 3, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Healthy);

            // At min throughput - unhealthy with high failure rate
            let info = HealthInfo::new(1, 4, 0.5, 5);
            assert_eq!(info.status(), HealthStatus::Unhealthy);
        }

        #[test]
        fn edge_cases() {
            // Saturating add
            let info = HealthInfo::new(u32::MAX, 1, 0.5, 5);
            assert_eq!(info.throughput(), u32::MAX);

            // Zero threshold
            let info = HealthInfo::new(1, 1, 0.0, 0);
            assert_eq!(info.status(), HealthStatus::Unhealthy);
        }
    }
}
