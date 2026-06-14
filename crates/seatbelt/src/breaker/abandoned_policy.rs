// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Controls how *abandoned* executions influence the circuit breaker's health decision.
///
/// An execution is *abandoned* when it is accepted by the circuit breaker but its future is dropped
/// before completing — for example when the caller cancels the request. Abandoned executions are
/// **always** counted towards the reported throughput for telemetry purposes; this policy only
/// governs whether, and how, they contribute to the open/close decision.
///
/// Three policies are available:
///
/// - [`AbandonedPolicy::ignore`]: abandoned executions never affect the decision.
/// - [`AbandonedPolicy::pathological`]: abandoned executions only affect the decision in the
///   degenerate case where there were no conclusive results at all (the default).
/// - [`AbandonedPolicy::as_failures`]: abandoned executions are always treated as failures.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(transparent))]
pub struct AbandonedPolicy {
    inner: Mode,
}

impl AbandonedPolicy {
    /// Abandoned executions never influence the open/close decision.
    ///
    /// They are still counted towards the reported throughput for telemetry, but they neither raise
    /// nor lower the failure rate and never count towards the minimum throughput required to open
    /// the circuit. Use this when cancellations are routine and should never be interpreted as a
    /// signal about the health of the underlying service.
    ///
    /// > **Note**: with this policy the pathological case where *every* execution is abandoned can
    /// > never open the circuit, because no conclusive result is ever observed.
    #[must_use]
    pub fn ignore() -> Self {
        Self { inner: Mode::Ignore }
    }

    /// **Default.** Abandoned executions only influence the decision in the pathological case where
    /// there were no successes and no failures — that is, every execution was abandoned.
    ///
    /// In that degenerate case the abandoned executions are treated as failures so the circuit can
    /// still react; otherwise it would never observe any result and could never open. As soon as
    /// there is at least one conclusive result (a success or a failure), abandoned executions are
    /// ignored entirely and the decision is made purely on successes and failures. This keeps
    /// abandoned executions from either masking a genuine failure burst or manufacturing a false
    /// failure rate, while still guarding against the "everything is abandoned" deadlock.
    #[must_use]
    pub fn pathological() -> Self {
        Self { inner: Mode::Pathological }
    }

    /// Abandoned executions are always treated as failures.
    ///
    /// Each abandoned execution contributes to both the numerator and the denominator of the
    /// failure rate, exactly as a real failure would. Use this when an abandoned execution should be
    /// considered just as bad as an outright failure (for example when cancellations are typically
    /// caused by the downstream service being too slow).
    #[must_use]
    pub fn as_failures() -> Self {
        Self { inner: Mode::AsFailures }
    }

    /// Computes the `(failures, total)` pair used for the health decision from the raw execution
    /// counts, according to this policy.
    ///
    /// The returned values are the failure count and the total count that the failure rate and the
    /// minimum-throughput check are evaluated against. They deliberately may differ from the
    /// reported throughput, which always includes abandoned executions.
    pub(crate) fn decision(&self, successes: u32, failures: u32, abandoned: u32) -> (u32, u32) {
        match self.inner {
            Mode::Ignore => (failures, successes.saturating_add(failures)),
            Mode::Pathological => {
                if successes == 0 && failures == 0 {
                    (abandoned, abandoned)
                } else {
                    (failures, successes.saturating_add(failures))
                }
            }
            Mode::AsFailures => (
                failures.saturating_add(abandoned),
                successes.saturating_add(failures).saturating_add(abandoned),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
enum Mode {
    #[default]
    Pathological,
    Ignore,
    AsFailures,
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_excludes_abandoned_from_decision() {
        let policy = AbandonedPolicy::ignore();
        assert_eq!(policy.decision(5, 1, 10), (1, 6));
        // Every execution abandoned: nothing conclusive, so the decision total is zero.
        assert_eq!(policy.decision(0, 0, 10), (0, 0));
    }

    #[test]
    fn pathological_considers_abandoned_only_when_all_abandoned() {
        let policy = AbandonedPolicy::pathological();
        assert_eq!(policy.decision(0, 0, 10), (10, 10));
        assert_eq!(policy.decision(1, 0, 10), (0, 1));
        assert_eq!(policy.decision(0, 2, 10), (2, 2));
    }

    #[test]
    fn as_failures_always_counts_abandoned() {
        let policy = AbandonedPolicy::as_failures();
        assert_eq!(policy.decision(5, 1, 10), (11, 16));
        assert_eq!(policy.decision(0, 0, 10), (10, 10));
    }

    #[test]
    fn default_is_pathological() {
        assert_eq!(AbandonedPolicy::default(), AbandonedPolicy::pathological());
    }
}
