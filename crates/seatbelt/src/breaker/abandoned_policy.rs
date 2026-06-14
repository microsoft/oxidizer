// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Controls how *abandoned* executions influence the circuit breaker's health decision.
///
/// An execution is *abandoned* when it is accepted by the circuit breaker but its future is dropped
/// before completing — for example when the caller cancels the request. Abandoned executions are
/// **always** counted towards the reported throughput for telemetry purposes; this policy only
/// governs whether, and how, they contribute to the open/close decision.
///
/// The following policies are available:
///
/// - [`AbandonedPolicy::ignore`]: abandoned executions never affect the decision.
/// - [`AbandonedPolicy::abandon_rate_threshold`]: abandoned executions are treated as failures once
///   the proportion of abandoned executions reaches a configured threshold.
/// - [`AbandonedPolicy::when_all_abandoned`]: the special case of `abandon_rate_threshold` with a
///   threshold of `1.0` — abandoned executions only affect the decision when *every* execution was
///   abandoned (the default).
/// - [`AbandonedPolicy::as_failures`]: abandoned executions are always treated as failures
///   (equivalent to `abandon_rate_threshold` with a threshold of `0.0`).
#[derive(Debug, Clone, PartialEq, Default)]
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
    /// > **Note**: with this policy the degenerate case where *every* execution is abandoned can
    /// > never open the circuit, because no conclusive result is ever observed.
    #[must_use]
    pub fn ignore() -> Self {
        Self { inner: Mode::Ignore }
    }

    /// **Default.** Abandoned executions only influence the decision when there were no successes and
    /// no failures — that is, every execution was abandoned.
    ///
    /// In that degenerate case the abandoned executions are treated as failures so the circuit can
    /// still react; otherwise it would never observe any result and could never open. As soon as
    /// there is at least one conclusive result (a success or a failure), abandoned executions are
    /// ignored entirely and the decision is made purely on successes and failures. This keeps
    /// abandoned executions from either masking a genuine failure burst or manufacturing a false
    /// failure rate, while still guarding against the "everything is abandoned" deadlock.
    ///
    /// This is exactly [`abandon_rate_threshold`][AbandonedPolicy::abandon_rate_threshold] with a
    /// threshold of `1.0`.
    #[must_use]
    pub fn when_all_abandoned() -> Self {
        Self::abandon_rate_threshold(1.0)
    }

    /// Abandoned executions are treated as failures once their proportion of the total throughput
    /// reaches `threshold`.
    ///
    /// The *abandon rate* is `abandoned / total`, where `total` includes successes, failures and
    /// abandoned executions. When the abandon rate is greater than or equal to `threshold`, every
    /// abandoned execution is counted as a failure (contributing to both the failure count and the
    /// total the failure rate is evaluated against), so a high enough rate of cancellations can on
    /// its own drive the health to *unhealthy*. While the abandon rate stays below `threshold`,
    /// abandoned executions are ignored entirely and the decision is made purely on successes and
    /// failures.
    ///
    /// `threshold` is a rate in `[0.0, 1.0]`:
    ///
    /// - `1.0` is equivalent to [`when_all_abandoned`][AbandonedPolicy::when_all_abandoned]:
    ///   abandoned executions only matter when *every* execution was abandoned.
    /// - `0.0` is equivalent to [`as_failures`][AbandonedPolicy::as_failures]: abandoned executions
    ///   are always treated as failures.
    ///
    /// # Panics
    ///
    /// Panics if `threshold` is not in `[0.0, 1.0]`.
    #[must_use]
    pub fn abandon_rate_threshold(threshold: f32) -> Self {
        assert!((0.0..=1.0).contains(&threshold), "threshold must be in [0.0, 1.0]");

        // A threshold of `0.0` means "any abandon rate counts", which is exactly `as_failures`.
        let inner = if threshold == 0.0 {
            Mode::AsFailures
        } else {
            Mode::AbandonRateThreshold(threshold)
        };

        Self { inner }
    }

    /// Abandoned executions are always treated as failures.
    ///
    /// Each abandoned execution contributes to both the numerator and the denominator of the
    /// failure rate, exactly as a real failure would. Use this when an abandoned execution should be
    /// considered just as bad as an outright failure (for example when cancellations are typically
    /// caused by the downstream service being too slow).
    ///
    /// This is exactly [`abandon_rate_threshold`][AbandonedPolicy::abandon_rate_threshold] with a
    /// threshold of `0.0`.
    #[must_use]
    pub fn as_failures() -> Self {
        Self { inner: Mode::AsFailures }
    }

    /// Returns `true` if abandoned executions are unconditionally treated as failures.
    ///
    /// This is used by the single-probe recovery gate, which has no statistical sample to apply
    /// the abandon-rate heuristic to: a lone abandoned probe is only conclusive evidence of failure
    /// under the [`as_failures`][AbandonedPolicy::as_failures] policy (equivalently
    /// [`abandon_rate_threshold(0.0)`][AbandonedPolicy::abandon_rate_threshold]).
    pub(crate) fn counts_abandoned_as_failure(&self) -> bool {
        matches!(self.inner, Mode::AsFailures)
    }

    /// Returns the policy's abandoned-handling [`Mode`] for the centralized health evaluator.
    ///
    /// This exposes the policy purely as configuration data: the actual `(failures, total)`
    /// derivation lives in [`HealthEvaluator`][super::HealthEvaluator], so that all health
    /// evaluation logic is centralized in one place rather than split across types.
    pub(crate) fn mode(&self) -> Mode {
        self.inner
    }
}

/// How abandoned executions are folded into the health decision.
///
/// This doubles as the serialization form of [`AbandonedPolicy`] and as the input the
/// [`HealthEvaluator`][super::HealthEvaluator] matches on when deriving a verdict.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
pub(crate) enum Mode {
    /// Abandoned executions never influence the decision.
    Ignore,
    /// Abandoned executions are always counted as failures.
    AsFailures,
    /// Abandoned executions are counted as failures once the abandon rate reaches this threshold.
    AbandonRateThreshold(f32),
}

impl Default for Mode {
    fn default() -> Self {
        // The default `when_all_abandoned` policy: abandoned executions only matter when every
        // execution was abandoned.
        Self::AbandonRateThreshold(1.0)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_maps_to_ignore_mode() {
        assert_eq!(AbandonedPolicy::ignore().mode(), Mode::Ignore);
    }

    #[test]
    fn when_all_abandoned_maps_to_rate_threshold_one() {
        assert_eq!(AbandonedPolicy::when_all_abandoned().mode(), Mode::AbandonRateThreshold(1.0));
    }

    #[test]
    fn as_failures_maps_to_as_failures_mode() {
        assert_eq!(AbandonedPolicy::as_failures().mode(), Mode::AsFailures);
    }

    #[test]
    fn abandon_rate_threshold_maps_to_rate_threshold_mode() {
        assert_eq!(AbandonedPolicy::abandon_rate_threshold(0.5).mode(), Mode::AbandonRateThreshold(0.5));
    }

    #[test]
    fn when_all_abandoned_equals_abandon_rate_threshold_one() {
        assert_eq!(AbandonedPolicy::when_all_abandoned(), AbandonedPolicy::abandon_rate_threshold(1.0));
    }

    #[test]
    fn as_failures_equals_abandon_rate_threshold_zero() {
        assert_eq!(AbandonedPolicy::as_failures(), AbandonedPolicy::abandon_rate_threshold(0.0));
    }

    #[test]
    fn abandon_rate_threshold_does_not_count_lone_probe_as_failure() {
        // Intermediate thresholds (and the 1.0 special case) leave the single-probe gate inconclusive.
        assert!(!AbandonedPolicy::abandon_rate_threshold(0.5).counts_abandoned_as_failure());
        assert!(!AbandonedPolicy::when_all_abandoned().counts_abandoned_as_failure());
        // A zero threshold is the `as_failures` policy, which does reopen on a lone abandoned probe.
        assert!(AbandonedPolicy::abandon_rate_threshold(0.0).counts_abandoned_as_failure());
    }

    #[test]
    #[should_panic(expected = "threshold must be in [0.0, 1.0]")]
    fn abandon_rate_threshold_rejects_out_of_range() {
        let _ = AbandonedPolicy::abandon_rate_threshold(1.5);
    }

    #[test]
    fn default_is_when_all_abandoned() {
        assert_eq!(AbandonedPolicy::default(), AbandonedPolicy::when_all_abandoned());
    }
}
