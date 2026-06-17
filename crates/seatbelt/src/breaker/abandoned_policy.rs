// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Controls how *abandoned* executions influence the circuit breaker's health decision.
///
/// An execution is *abandoned* when the circuit breaker accepts it but its future is dropped before
/// completing — for example when the caller cancels the request. Abandoned executions are **always**
/// counted towards the reported throughput for telemetry; this policy only governs whether, and how,
/// they contribute to the open/close decision.
///
/// - [`ignore`][AbandonedPolicy::ignore]: abandoned executions never affect the decision.
/// - [`abandon_rate_threshold`][AbandonedPolicy::abandon_rate_threshold]: abandoned executions count
///   as failures once their proportion of the throughput reaches a threshold.
/// - [`when_all_abandoned`][AbandonedPolicy::when_all_abandoned]: the default — `abandon_rate_threshold`
///   with a threshold of `1.0`, so abandoned executions only matter when *every* execution was abandoned.
/// - [`as_failures`][AbandonedPolicy::as_failures]: abandoned executions always count as failures.
///
/// # Why cancel safety matters
///
/// Consider a hedging strategy across two endpoints. When the first endpoint becomes unresponsive,
/// every hedged attempt is served by the second endpoint and the in-flight attempt to the first
/// endpoint is cancelled -- its future dropped before completing -- rather than returning a result.
/// Without a policy that can treat those cancellations as conclusive, the first endpoint's circuit
/// only ever observes abandoned executions, never a success or failure, so it never opens. It keeps
/// admitting doomed attempts, wasting resources on an endpoint that is effectively down. The default
/// [`when_all_abandoned`][AbandonedPolicy::when_all_abandoned] policy lets a run of purely abandoned
/// executions open the circuit, so the unhealthy endpoint is shed instead of retried indefinitely.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(transparent))]
pub struct AbandonedPolicy {
    inner: Mode,
}

impl AbandonedPolicy {
    /// Abandoned executions never influence the open/close decision.
    ///
    /// They are still reported in throughput telemetry, but never affect the failure rate or the
    /// minimum throughput. Use this when cancellations are routine and say nothing about the health
    /// of the underlying service.
    ///
    /// > **Note**: with this policy a run of *only* abandoned executions can never open the circuit,
    /// > because no conclusive result is ever observed.
    #[must_use]
    pub fn ignore() -> Self {
        Self { inner: Mode::Ignore }
    }

    /// **Default.** Abandoned executions only count when *every* execution was abandoned.
    ///
    /// As long as there is at least one conclusive result (a success or a failure), abandoned
    /// executions are ignored and the decision is made purely on successes and failures. Only in the
    /// degenerate case where everything was abandoned do they count as failures, so the circuit can
    /// still react instead of deadlocking on a service that never returns a result.
    ///
    /// This is [`abandon_rate_threshold`][AbandonedPolicy::abandon_rate_threshold] with a threshold
    /// of `1.0`.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Equivalent mutant: Default::default() resolves to Mode::AbandonRateThreshold(1.0), the same value returned here.
    pub fn when_all_abandoned() -> Self {
        Self::abandon_rate_threshold(1.0)
    }

    /// Abandoned executions count as failures once their share of the throughput reaches `threshold`.
    ///
    /// The *abandon rate* is `abandoned / total`, where `total` counts successes, failures and
    /// abandoned executions. Once the abandon rate reaches `threshold`, each abandoned execution is
    /// folded into the failure rate (counting towards both its numerator and denominator), so enough
    /// cancellations can open the circuit on their own. While the abandon rate stays below
    /// `threshold`, abandoned executions are ignored and the decision rests on successes and failures
    /// alone.
    ///
    /// `threshold` is a rate in `(0.0, 1.0]`; `1.0` is equivalent to
    /// [`when_all_abandoned`][AbandonedPolicy::when_all_abandoned]. To count abandoned executions as
    /// failures unconditionally, use [`as_failures`][AbandonedPolicy::as_failures] instead.
    ///
    /// # Panics
    ///
    /// Panics if `threshold` is not in `(0.0, 1.0]`.
    #[must_use]
    pub fn abandon_rate_threshold(threshold: f32) -> Self {
        assert!(threshold > 0.0 && threshold <= 1.0, "threshold must be in (0.0, 1.0]");

        Self {
            inner: Mode::AbandonRateThreshold(threshold),
        }
    }

    /// Abandoned executions always count as failures.
    ///
    /// Each abandoned execution contributes to both the numerator and the denominator of the failure
    /// rate, exactly as a real failure would. Use this when an abandoned execution is just as bad as
    /// an outright failure — for example when cancellations are typically caused by a downstream
    /// service being too slow.
    #[must_use]
    pub fn as_failures() -> Self {
        Self { inner: Mode::AsFailures }
    }

    /// Returns `true` if abandoned executions are unconditionally treated as failures.
    ///
    /// Used by the single-probe recovery gate, which has no statistical sample to apply the
    /// abandon-rate heuristic to: a lone abandoned probe is only conclusive evidence of failure under
    /// the [`as_failures`][AbandonedPolicy::as_failures] policy.
    pub(crate) fn counts_abandoned_as_failure(&self) -> bool {
        matches!(self.inner, Mode::AsFailures)
    }

    /// Returns the policy's abandoned-handling [`Mode`] for the centralized health evaluator.
    ///
    /// The `(failures, total)` derivation lives in [`HealthEvaluator`][super::HealthEvaluator], so
    /// this exposes the policy purely as configuration data.
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
    AbandonRateThreshold(#[cfg_attr(any(feature = "serde", test), serde(deserialize_with = "coerce_abandon_rate_threshold"))] f32),
}

impl Default for Mode {
    fn default() -> Self {
        // The default `when_all_abandoned` policy: abandoned executions only matter when every
        // execution was abandoned.
        Self::AbandonRateThreshold(1.0)
    }
}

/// Coerces a deserialized abandon-rate threshold into the valid `(0.0, 1.0]` range.
///
/// Hand-written or generated configuration can carry an out-of-range threshold that the
/// [`abandon_rate_threshold`][AbandonedPolicy::abandon_rate_threshold] constructor would reject.
/// Rather than failing to deserialize, the value is clamped into the valid range: values above
/// `1.0` collapse to `1.0`, and values at or below `0.0` collapse to the smallest positive `f32`
/// so the exclusive lower bound is preserved.
#[cfg(any(feature = "serde", test))]
fn coerce_abandon_rate_threshold<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let threshold = <f32 as serde::Deserialize>::deserialize(deserializer)?;
    Ok(threshold.clamp(f32::MIN_POSITIVE, 1.0))
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
    fn abandon_rate_threshold_does_not_count_lone_probe_as_failure() {
        // Intermediate thresholds (and the 1.0 special case) leave the single-probe gate inconclusive.
        assert!(!AbandonedPolicy::abandon_rate_threshold(0.5).counts_abandoned_as_failure());
        assert!(!AbandonedPolicy::when_all_abandoned().counts_abandoned_as_failure());
        // The `as_failures` policy does reopen on a lone abandoned probe.
        assert!(AbandonedPolicy::as_failures().counts_abandoned_as_failure());
    }

    #[test]
    #[should_panic(expected = "threshold must be in (0.0, 1.0]")]
    fn abandon_rate_threshold_rejects_above_range() {
        let _ = AbandonedPolicy::abandon_rate_threshold(1.5);
    }

    #[test]
    #[should_panic(expected = "threshold must be in (0.0, 1.0]")]
    fn abandon_rate_threshold_rejects_zero() {
        let _ = AbandonedPolicy::abandon_rate_threshold(0.0);
    }

    #[test]
    fn default_is_when_all_abandoned() {
        assert_eq!(AbandonedPolicy::default(), AbandonedPolicy::when_all_abandoned());
    }

    #[test]
    fn deserialize_preserves_valid_threshold() {
        let policy: AbandonedPolicy = serde_json::from_str(r#"{"AbandonRateThreshold":0.5}"#).unwrap();
        assert_eq!(policy.mode(), Mode::AbandonRateThreshold(0.5));
    }

    #[test]
    fn deserialize_coerces_threshold_above_one_to_one() {
        let policy: AbandonedPolicy = serde_json::from_str(r#"{"AbandonRateThreshold":1.5}"#).unwrap();
        assert_eq!(policy.mode(), Mode::AbandonRateThreshold(1.0));
    }

    #[test]
    fn deserialize_coerces_non_positive_threshold_to_smallest_positive() {
        let zero: AbandonedPolicy = serde_json::from_str(r#"{"AbandonRateThreshold":0.0}"#).unwrap();
        assert_eq!(zero.mode(), Mode::AbandonRateThreshold(f32::MIN_POSITIVE));

        let negative: AbandonedPolicy = serde_json::from_str(r#"{"AbandonRateThreshold":-0.5}"#).unwrap();
        assert_eq!(negative.mode(), Mode::AbandonRateThreshold(f32::MIN_POSITIVE));
    }
}
