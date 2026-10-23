// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Controls how abandoned executions affect circuit health.
///
/// An execution is abandoned when its future is dropped before completion.
/// For example, a caller can cancel the request.
/// Telemetry always includes abandoned executions in throughput.
/// This policy controls only the health decision.
///
/// - [`ignore`][AbandonedPolicy::ignore] does not use abandoned executions in the decision.
/// - [`rate_threshold`][AbandonedPolicy::rate_threshold] uses them after their rate reaches a threshold.
/// - [`as_failures`][AbandonedPolicy::as_failures] always counts them as failures.
///
/// # Canceled hedged calls
///
/// A hedged call can cancel a slow attempt after another attempt completes.
/// The slow endpoint then produces only abandoned executions.
/// The default `rate_threshold(1.0)` policy can open its circuit when all executions are abandoned.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(transparent))]
pub struct AbandonedPolicy {
    inner: Mode,
}

impl AbandonedPolicy {
    /// Ignores abandoned executions in the health decision.
    ///
    /// Telemetry still includes them in throughput.
    /// Use this policy when cancellation does not show service health.
    /// If all executions are abandoned, this policy cannot open the circuit.
    #[must_use]
    pub fn ignore() -> Self {
        Self { inner: Mode::Ignore }
    }

    /// Counts abandoned executions as failures when their rate reaches `threshold`.
    ///
    /// The abandon rate is `abandoned / total`.
    /// The total includes successful, failed, and abandoned executions.
    /// Below the threshold, the health decision ignores abandoned executions.
    ///
    /// `threshold` must be in `(0.0, 1.0]`.
    /// The default is `1.0`.
    /// Use [`as_failures`][AbandonedPolicy::as_failures] to count every abandoned execution as a failure.
    ///
    /// # Panics
    ///
    /// Panics if `threshold` is not in `(0.0, 1.0]`.
    #[must_use]
    pub fn rate_threshold(threshold: f32) -> Self {
        assert!(threshold > 0.0 && threshold <= 1.0, "threshold must be in (0.0, 1.0]");

        Self {
            inner: Mode::RateThreshold(threshold),
        }
    }

    /// Counts every abandoned execution as a failure.
    ///
    /// Use this policy when cancellation usually means that the service is too slow.
    #[must_use]
    pub fn as_failures() -> Self {
        Self { inner: Mode::AsFailures }
    }

    /// Returns `true` when all abandoned executions count as failures.
    ///
    /// A single probe does not provide a statistical sample.
    /// Only [`as_failures`][AbandonedPolicy::as_failures] treats one abandoned probe as a failure.
    pub(crate) fn counts_abandoned_as_failure(&self) -> bool {
        matches!(self.inner, Mode::AsFailures)
    }

    /// Returns the [`Mode`] for [`HealthEvaluator`][super::HealthEvaluator].
    pub(crate) fn mode(&self) -> Mode {
        self.inner
    }
}

/// Defines how abandoned executions affect the health decision.
///
/// This is also the serialized form of [`AbandonedPolicy`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(rename_all = "snake_case"))]
pub(crate) enum Mode {
    /// Abandoned executions never influence the decision.
    Ignore,
    /// Abandoned executions are always counted as failures.
    AsFailures,
    /// Abandoned executions are counted as failures once the abandon rate reaches this threshold.
    RateThreshold(#[cfg_attr(any(feature = "serde", test), serde(deserialize_with = "coerce_rate_threshold"))] f32),
}

impl Default for Mode {
    fn default() -> Self {
        // The default `rate_threshold(1.0)` policy: abandoned executions only matter when every
        // execution was abandoned.
        Self::RateThreshold(1.0)
    }
}

/// Limits a deserialized abandon-rate threshold to `(0.0, 1.0]`.
///
/// Configuration can contain a value that the constructor rejects.
/// Values above `1.0` become `1.0`.
/// Values at or below `0.0` become the smallest positive `f32`.
#[cfg(any(feature = "serde", test))]
fn coerce_rate_threshold<'de, D>(deserializer: D) -> Result<f32, D::Error>
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
    fn as_failures_maps_to_as_failures_mode() {
        assert_eq!(AbandonedPolicy::as_failures().mode(), Mode::AsFailures);
    }

    #[test]
    fn rate_threshold_maps_to_rate_threshold_mode() {
        assert_eq!(AbandonedPolicy::rate_threshold(0.5).mode(), Mode::RateThreshold(0.5));
    }

    #[test]
    fn rate_threshold_does_not_count_lone_probe_as_failure() {
        // Intermediate thresholds (and the 1.0 special case) leave the single-probe gate inconclusive.
        assert!(!AbandonedPolicy::rate_threshold(0.5).counts_abandoned_as_failure());
        assert!(!AbandonedPolicy::rate_threshold(1.0).counts_abandoned_as_failure());
        // The `as_failures` policy does reopen on a lone abandoned probe.
        assert!(AbandonedPolicy::as_failures().counts_abandoned_as_failure());
    }
}
