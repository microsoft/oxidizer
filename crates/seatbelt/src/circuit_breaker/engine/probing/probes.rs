// Copyright (c) Microsoft Corporation.

use std::time::Instant;
use std::vec;

use super::{AllowProbeResult, Probe, ProbeOperation, ProbeOptions, ProbesOptions, ProbingResult};
use crate::circuit_breaker::ExecutionResult;

/// Manages a sequence of probes.
#[derive(Debug)]
pub(crate) struct Probes {
    probes: vec::IntoIter<ProbeOptions>,
    current: Probe,
}

impl Probes {
    pub fn new(options: &ProbesOptions) -> Self {
        let mut probes = options.probes();
        let probe = probes.next().expect("probes are never empty because ProbesOptions enforces that");

        Self {
            probes,
            current: Probe::new(probe),
        }
    }

    pub fn allow_probe(&mut self, now: Instant) -> AllowProbeResult {
        self.current.allow_probe(now)
    }

    pub fn record(&mut self, result: ExecutionResult, now: Instant) -> ProbingResult {
        match self.current.record(result, now) {
            ProbingResult::Success => {
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

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use super::*;

    #[test]
    fn multiple_probes_ok() {
        let options = ProbesOptions::new([
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(1),
            },
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(2),
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
}
