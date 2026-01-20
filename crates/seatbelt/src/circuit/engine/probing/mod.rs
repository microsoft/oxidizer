// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Probing mechanisms for circuit breakers.
//!
//! Probing is used to test if a service has recovered after a failure.
//! Different probing strategies can be implemented by implementing the `ProbeOperation` trait.
//!
//! - Various probes can be combined in sequence using the [`Probes`] struct.
//! - Unified view over various probe types is provided by the [`Probe`] enum.

use std::fmt::Debug;
use std::time::Instant;

use crate::circuit::{EnterCircuitResult, ExecutionMode, ExecutionResult};

mod health_probe;
mod options;
mod probes;
mod single_probe;

pub(crate) use health_probe::*;
pub(crate) use options::*;
pub(crate) use probes::*;
pub(crate) use single_probe::*;

/// Result of a probing attempt.
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum ProbingResult {
    /// Probing succeeded, no more probing needed.
    Success,

    /// Probing failed, circuit should remain open.
    Failure,

    /// Probing is still in progress, more probes are needed.
    Pending,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum AllowProbeResult {
    Accepted,
    Rejected,
}

impl From<AllowProbeResult> for EnterCircuitResult {
    fn from(value: AllowProbeResult) -> Self {
        match value {
            AllowProbeResult::Accepted => Self::Accepted {
                mode: ExecutionMode::Probe,
            },
            AllowProbeResult::Rejected => Self::Rejected,
        }
    }
}

/// Trait defining the behavior of a probing mechanism in a circuit breaker.
pub(crate) trait ProbeOperation: Send + Sync + Debug + 'static {
    fn allow_probe(&mut self, now: Instant) -> AllowProbeResult;

    fn record(&mut self, result: ExecutionResult, now: Instant) -> ProbingResult;
}

/// View over multiple probe types.
#[derive(Debug)]
pub(crate) enum Probe {
    Single(SingleProbe),
    Health(HealthProbe),
}

impl Probe {
    pub fn new(options: ProbeOptions) -> Self {
        match options {
            ProbeOptions::SingleProbe { cooldown } => Self::Single(SingleProbe::new(cooldown)),
            ProbeOptions::HealthProbe(options) => Self::Health(HealthProbe::new(options)),
        }
    }
}

impl ProbeOperation for Probe {
    fn allow_probe(&mut self, now: Instant) -> AllowProbeResult {
        match self {
            Self::Single(probe) => probe.allow_probe(now),
            Self::Health(health) => health.allow_probe(now),
        }
    }

    /// Record the result of a probing attempt.
    ///
    /// Once the probe reports success or failure, it is considered complete and
    /// should never be used again.
    fn record(&mut self, result: ExecutionResult, now: Instant) -> ProbingResult {
        match self {
            Self::Single(probe) => probe.record(result, now),
            Self::Health(health) => health.record(result, now),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn probe_new_creates_single_probe() {
        let cooldown = Duration::from_secs(5);
        let probe = Probe::new(ProbeOptions::SingleProbe { cooldown });
        assert!(matches!(probe, Probe::Single(duration) if duration.probe_cooldown() == cooldown));
    }

    #[test]
    fn probe_allow_probe_delegates_to_inner() {
        let mut probe = Probe::new(ProbeOptions::SingleProbe {
            cooldown: Duration::from_secs(5),
        });
        let now = Instant::now();

        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);
        assert_eq!(probe.allow_probe(now), AllowProbeResult::Rejected);
    }

    #[test]
    fn probe_record_delegates_to_inner() {
        let mut probe = Probe::new(ProbeOptions::SingleProbe {
            cooldown: Duration::from_secs(5),
        });
        let now = Instant::now();

        assert_eq!(probe.record(ExecutionResult::Success, now), ProbingResult::Success);
        assert_eq!(probe.record(ExecutionResult::Failure, now), ProbingResult::Failure);
    }

    #[test]
    fn allow_probe_result_to_enter_circuit_result_ok() {
        assert!(matches!(
            EnterCircuitResult::from(AllowProbeResult::Accepted),
            EnterCircuitResult::Accepted {
                mode: ExecutionMode::Probe
            }
        ));

        assert!(matches!(
            EnterCircuitResult::from(AllowProbeResult::Rejected),
            EnterCircuitResult::Rejected
        ));
    }

    #[test]
    fn probe_new_creates_health_probe() {
        let options = HealthProbeOptions::new(Duration::from_secs(10), 0.2, 0.5);
        let probe = Probe::new(ProbeOptions::HealthProbe(options));
        assert!(matches!(probe, Probe::Health(_)));
    }

    #[test]
    fn probe_health_allow_probe_delegates_to_inner() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.2, 1.0);
        let mut probe = Probe::new(ProbeOptions::HealthProbe(options));
        let now = Instant::now();

        // With probing_ratio=1.0, all probes should be accepted
        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);
    }

    #[test]
    fn probe_health_record_delegates_to_inner() {
        let options = HealthProbeOptions::new(Duration::from_secs(5), 0.2, 1.0);
        let mut probe = Probe::new(ProbeOptions::HealthProbe(options));
        let now = Instant::now();

        // allow_probe initializes the sampling period
        assert_eq!(probe.allow_probe(now), AllowProbeResult::Accepted);

        // Record before sampling period ends returns Pending
        assert_eq!(probe.record(ExecutionResult::Success, now), ProbingResult::Pending);

        // Record after sampling period with success returns Success
        assert_eq!(
            probe.record(ExecutionResult::Success, now + Duration::from_secs(5)),
            ProbingResult::Success
        );
    }
}
