// Copyright (c) Microsoft Corporation.

use std::time::{Duration, Instant};

use super::{AllowProbeResult, ProbeOperation, ProbingResult};
use crate::circuit::ExecutionResult;

/// Allows a single probe to get in and based on the result either closes the circuit
/// or goes back to open state.
#[derive(Debug, Clone)]
pub(crate) struct SingleProbe {
    probe_cooldown: Duration,
    entered_at: Option<Instant>,
}

impl SingleProbe {
    pub fn new(probe_cooldown: Duration) -> Self {
        Self {
            probe_cooldown,
            entered_at: None,
        }
    }

    #[cfg(test)]
    pub fn probe_cooldown(&self) -> Duration {
        self.probe_cooldown
    }
}

impl ProbeOperation for SingleProbe {
    fn allow_probe(&mut self, now: Instant) -> AllowProbeResult {
        match self.entered_at {
            // First probe attempt - record the timestamp to start the cool-down period
            None => {
                self.entered_at = Some(now);
                AllowProbeResult::Accepted
            }
            // Cool-down has elapsed, allow the probe and reset the cool-down timer.
            // We allow additional probe after the cool-down period to handle the case
            // where the probe result is not recorded due to future being dropped.
            Some(entered_at) if now.saturating_duration_since(entered_at) > self.probe_cooldown => {
                self.entered_at = Some(now);
                AllowProbeResult::Accepted
            }
            Some(_) => AllowProbeResult::Rejected,
        }
    }

    fn record(&mut self, result: ExecutionResult, _now: Instant) -> ProbingResult {
        match result {
            ExecutionResult::Success => ProbingResult::Success,
            ExecutionResult::Failure => ProbingResult::Failure,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_probe_accepts_single_probe() {
        let mut probe = SingleProbe::new(Duration::from_secs(5));
        let now = Instant::now();

        // The first probe should be accepted
        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);

        // The second probe immediately should be rejected
        assert_eq!(probe.allow_probe(now), AllowProbeResult::Rejected);

        // After 3 seconds, still should be rejected
        let later = now + Duration::from_secs(3);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Rejected);

        // After cooldown, the probe should be accepted again
        let later = now + Duration::from_secs(6);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Accepted);
    }

    #[test]
    fn allow_probe_check_bounds() {
        let mut probe = SingleProbe::new(Duration::from_secs(5));
        let now = Instant::now();

        // The first probe should be accepted
        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);

        // After exactly cool-down duration, the probe should still be rejected
        let later = now + Duration::from_secs(5);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Rejected);

        // After cool-down + 1 microsecond, the probe should be accepted
        let later = now + Duration::from_secs(5) + Duration::from_micros(1);
        assert_eq!(probe.allow_probe(later), AllowProbeResult::Accepted);
    }

    #[test]
    fn record_ensure_correct_result() {
        let mut probe = SingleProbe::new(Duration::from_secs(5));
        let now = Instant::now();

        // Record a success
        assert_eq!(probe.record(ExecutionResult::Success, now), ProbingResult::Success);

        // Record a failure
        assert_eq!(probe.record(ExecutionResult::Failure, now), ProbingResult::Failure);
    }
}
