// Copyright (c) Microsoft Corporation.

use std::time::Instant;

use crate::circuit::engine::probing::{AllowProbeResult, HealthProbeOptions, ProbeOperation, ProbingResult};
use crate::circuit::{ExecutionResult, HealthMetrics, HealthStatus};
use crate::rnd::Rnd;

#[derive(Debug)]
pub(crate) struct HealthProbe {
    options: HealthProbeOptions,
    metrics: HealthMetrics,
    fallback_after: Option<Instant>,
    sample_until: Option<Instant>,
    rnd: Rnd,
}

impl ProbeOperation for HealthProbe {
    fn allow_probe(&mut self, now: Instant) -> AllowProbeResult {
        // Sampling starts with the first probe attempt. Make sure relevant timestamps are set.
        let sample_until = *self.sample_until.get_or_insert_with(|| now + self.options.stage_duration());

        // Fallback probe is allowed only after the sampling duration has elapsed.
        let fallback_after = *self.fallback_after.get_or_insert(sample_until);

        // Allow probe based on the probing ratio.
        if self.rnd.next_f64() < self.options.probing_ratio {
            return AllowProbeResult::Accepted;
        }

        // Allow fallback probe to get through if we are past the sampling duration.
        // This can happen if the traffic is very low and no probes were allowed
        // by the rate sampling. This allows making progress in low-traffic scenarios
        // as a last resort.
        if now > fallback_after {
            // Allow additional fallback probes only after another sampling duration
            // in case allowed probe did not result in a recorded execution (e.g., due to timeout).
            self.fallback_after = Some(now + self.options.stage_duration());
            return AllowProbeResult::Accepted;
        }

        AllowProbeResult::Rejected
    }

    fn record(&mut self, result: ExecutionResult, now: Instant) -> ProbingResult {
        // Always record the result
        self.metrics.record(result, now);

        // If we are still sampling, we cannot make a decision yet
        if self.keep_sampling(now) {
            return ProbingResult::Pending;
        }

        // Sampling duration elapsed, use the health metrics to determine the result
        match self.metrics.health_info().status() {
            HealthStatus::Healthy => ProbingResult::Success,
            HealthStatus::Unhealthy => ProbingResult::Failure,
        }
    }
}

impl HealthProbe {
    pub fn new(options: HealthProbeOptions) -> Self {
        Self {
            metrics: options.builder.build(),
            options,
            fallback_after: None,
            sample_until: None,
            rnd: Rnd::Real,
        }
    }

    fn keep_sampling(&self, now: Instant) -> bool {
        match self.sample_until {
            None => true,
            Some(until) if now < until => true,
            _ => false,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn allow_probe_fallback() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.5, 0.1);
        let mut probe = HealthProbe::new(options);
        probe.rnd = Rnd::new_fixed(0.5);
        let now = Instant::now();

        assert_eq!(probe.allow_probe(now), AllowProbeResult::Rejected);

        let later = now + Duration::from_secs(5);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Rejected);

        // Allowed, because we are past the sampling duration and no probes were allowed yet
        let later = now + Duration::from_secs(5) + Duration::from_micros(1);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Accepted);

        // Not allowed, because we already let the fallback probe through
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Rejected);

        // Allowed again
        let later = now + Duration::from_secs(10) + Duration::from_micros(2);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Accepted);
    }

    #[test]
    fn allow_probe_rejected_when_at_ratio() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.5, 0.1);
        let mut probe = HealthProbe::new(options);
        probe.rnd = Rnd::new_fixed(0.1);

        assert_eq!(probe.allow_probe(Instant::now()), AllowProbeResult::Rejected);
    }

    #[test]
    fn record_not_allowed_before() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.99, 0.1);
        let mut probe = HealthProbe::new(options);
        let now = Instant::now();

        assert_eq!(probe.record(ExecutionResult::Success, now), ProbingResult::Pending,);

        assert_eq!(probe.record(ExecutionResult::Success, now), ProbingResult::Pending,);

        let status = probe.metrics.health_info();
        assert_eq!(status.status(), HealthStatus::Healthy);
        assert_eq!(status.throughput(), 2);
    }

    #[test]
    fn allow_then_record_after_sampling_period_healthy() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.1, 1.0);
        let mut probe = HealthProbe::new(options);
        let now = Instant::now();

        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);

        // At the edge of a sampling period, success
        assert_eq!(
            probe.record(ExecutionResult::Success, now + Duration::from_secs(5)),
            ProbingResult::Success,
        );

        assert_eq!(
            probe.record(ExecutionResult::Success, now + Duration::from_secs(10)),
            ProbingResult::Success,
        );
    }

    #[test]
    fn allow_then_record_after_sampling_period_unhealthy() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.1, 1.0);
        let mut probe = HealthProbe::new(options);
        let now = Instant::now();

        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);
        assert_eq!(
            probe.record(ExecutionResult::Failure, now + Duration::from_secs(10)),
            ProbingResult::Failure,
        );
    }

    #[test]
    fn record_multiple_ensure_health_evaluated() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.6, 1.0);
        let mut probe = HealthProbe::new(options);
        let now = Instant::now();

        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);
        assert_eq!(
            probe.record(ExecutionResult::Success, now + Duration::from_secs(1)),
            ProbingResult::Pending,
        );
        assert_eq!(
            probe.record(ExecutionResult::Failure, now + Duration::from_secs(2)),
            ProbingResult::Pending,
        );
        assert_eq!(
            probe.record(ExecutionResult::Success, now + Duration::from_secs(6)),
            ProbingResult::Success,
        );

        assert_eq!(
            probe.record(ExecutionResult::Failure, now + Duration::from_secs(6)),
            ProbingResult::Success,
        );

        assert_eq!(
            probe.record(ExecutionResult::Failure, now + Duration::from_secs(6)),
            ProbingResult::Failure,
        );
    }
}
