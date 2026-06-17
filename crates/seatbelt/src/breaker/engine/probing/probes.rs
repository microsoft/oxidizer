// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Instant;
use std::vec;

use super::{AllowProbeResult, Probe, ProbeOperation, ProbeOptions, ProbesOptions, ProbingResult};
use crate::breaker::ExecutionResult;

/// Manages a sequence of probes.
#[derive(Debug)]
pub(crate) struct Probes {
    probes: vec::IntoIter<ProbeOptions>,
    current: Probe,
}

impl Probes {
    pub(crate) fn new(options: &ProbesOptions) -> Self {
        let mut probes = options.probes();
        let probe = probes.next().expect("probes are never empty because ProbesOptions enforces that");

        Self {
            probes,
            current: Probe::new(probe),
        }
    }

    pub(crate) fn allow_probe(&mut self, now: Instant) -> AllowProbeResult {
        self.current.allow_probe(now)
    }

    pub(crate) fn record(&mut self, result: ExecutionResult, now: Instant) -> ProbingResult {
        match self.current.record(result, now) {
            ProbingResult::Success => {
                // An abandoned execution (a dropped or cancelled future) is never conclusive
                // evidence of recovery, so it must never advance probing or close the circuit. It
                // is still recorded into the probe above -- so it counts toward the health sample
                // and can reopen under `AbandonedPolicy::as_failures` -- but the close is deferred
                // to the next conclusive probe.
                if result == ExecutionResult::Abandoned {
                    return ProbingResult::Pending;
                }

                // check if there are more probes to try
                match self.probes.next() {
                    Some(probe) => {
                        self.current = Probe::new(probe);
                        ProbingResult::Pending
                    }
                    None => ProbingResult::Success,
                }
            }
            ProbingResult::Pending => ProbingResult::Pending,
            ProbingResult::Failure => ProbingResult::Failure,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {

    use std::time::Duration;

    use tick::Clock;

    use super::*;
    use crate::breaker::AbandonedPolicy;
    use crate::breaker::engine::probing::HealthProbeOptions;

    #[test]
    fn multiple_probes_ok() {
        let options = ProbesOptions::new([
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(1),
                abandoned_policy: AbandonedPolicy::default(),
            },
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(2),
                abandoned_policy: AbandonedPolicy::default(),
            },
        ]);
        let mut probes = Probes::new(&options);
        let now = Instant::now();

        assert_eq!(probes.allow_probe(now), AllowProbeResult::Accepted);
        assert_eq!(probes.allow_probe(now), AllowProbeResult::Rejected);
        assert_eq!(probes.record(ExecutionResult::Success, now), ProbingResult::Pending);

        assert_eq!(probes.allow_probe(now), AllowProbeResult::Accepted);
        assert_eq!(probes.record(ExecutionResult::Success, now), ProbingResult::Success);

        assert!(probes.probes.next().is_none());
    }

    #[test]
    fn record_returns_pending_when_probe_returns_pending() {
        let now = Clock::new_frozen().instant();

        let options = ProbesOptions::new([ProbeOptions::HealthProbe(HealthProbeOptions::new(
            Duration::from_secs(5),
            0.2,
            1.0,
            AbandonedPolicy::default(),
        ))]);
        let mut probes = Probes::new(&options);

        // Initialize sampling period
        assert_eq!(probes.allow_probe(now), AllowProbeResult::Accepted);

        // Record during sampling period returns Pending
        assert_eq!(probes.record(ExecutionResult::Success, now), ProbingResult::Pending);
    }

    #[test]
    fn abandoned_result_never_closes_circuit() {
        let start = Clock::new_frozen().instant();

        let options = ProbesOptions::new([ProbeOptions::HealthProbe(HealthProbeOptions::new(
            Duration::from_secs(5),
            0.2,
            1.0,
            AbandonedPolicy::default(),
        ))]);
        let mut probes = Probes::new(&options);

        // Begin sampling and record a healthy result within the sampling window.
        assert_eq!(probes.allow_probe(start), AllowProbeResult::Accepted);
        assert_eq!(
            probes.record(ExecutionResult::Success, start + Duration::from_secs(1)),
            ProbingResult::Pending
        );

        // Once sampling has elapsed, an abandoned execution evaluates the (still healthy) sample as
        // a success, but it must never close the circuit: probing stays pending and defers to a
        // conclusive probe.
        let after_window = start + Duration::from_secs(5);
        assert_eq!(probes.record(ExecutionResult::Abandoned, after_window), ProbingResult::Pending);

        // A subsequent conclusive (successful) probe does close the circuit.
        assert_eq!(probes.record(ExecutionResult::Success, after_window), ProbingResult::Success);
    }

    #[test]
    fn abandoned_result_can_still_reopen_under_as_failures() {
        let start = Clock::new_frozen().instant();

        let options = ProbesOptions::new([ProbeOptions::HealthProbe(HealthProbeOptions::new(
            Duration::from_secs(5),
            0.2,
            1.0,
            AbandonedPolicy::as_failures(),
        ))]);
        let mut probes = Probes::new(&options);

        // Begin sampling, then evaluate after the window: under `as_failures` an abandoned
        // execution counts as a failure, so it can still reopen the circuit (the guard only
        // suppresses the *close* transition, never a failure).
        assert_eq!(probes.allow_probe(start), AllowProbeResult::Accepted);
        assert_eq!(probes.record(ExecutionResult::Abandoned, start), ProbingResult::Pending);

        let after_window = start + Duration::from_secs(6);
        assert_eq!(probes.record(ExecutionResult::Abandoned, after_window), ProbingResult::Failure);
    }
}
