// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::breaker::AbandonedPolicy;
use crate::breaker::constants::MIN_SAMPLING_DURATION;
use crate::breaker::engine::probing::ProbesOptions;

/// Controls how a half-open circuit tests service health.
///
/// A half-open circuit allows a limited number of calls.
/// These calls show if the service recovered.
///
/// - [`HalfOpenMode::quick`] uses one probe.
/// - [`HalfOpenMode::progressive`] increases the probe rate in stages.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(transparent))]
pub struct HalfOpenMode {
    inner: Mode,
}

impl HalfOpenMode {
    /// Uses one probe to decide if the circuit can close.
    ///
    /// This mode can close the circuit faster than [`HalfOpenMode::progressive`].
    /// One successful call can also hide a service that still fails.
    #[must_use]
    pub fn quick() -> Self {
        Self { inner: Mode::Quick }
    }

    /// Increases the probe rate in stages.
    ///
    /// The probe ratios are:
    /// `0.1%, 1%, 5%, 10%, 25%, 50%`
    ///
    /// A stage advances after its duration ends and its failure rate is below the threshold.
    /// If a stage fails, the circuit opens again.
    ///
    /// `stage_duration` sets the duration of each stage.
    /// If it is `None`, the mode uses [`break_duration`][crate::breaker::BreakerLayer::break_duration].
    /// Durations shorter than one second become one second.
    #[must_use]
    pub fn progressive(stage_duration: impl Into<Option<Duration>>) -> Self {
        Self {
            inner: Mode::Progressive(stage_duration.into().map(|d| d.max(MIN_SAMPLING_DURATION))),
        }
    }

    pub(super) fn to_options(
        &self,
        default_stage_duration: Duration,
        failure_threshold: f32,
        abandoned_policy: &AbandonedPolicy,
    ) -> ProbesOptions {
        match self.inner {
            Mode::Quick => ProbesOptions::quick(default_stage_duration, abandoned_policy),
            Mode::Progressive(duration) => {
                ProbesOptions::progressive(duration.unwrap_or(default_stage_duration), failure_threshold, abandoned_policy)
            }
        }
    }
}

impl Default for HalfOpenMode {
    /// Returns [`HalfOpenMode::progressive(None)`](HalfOpenMode::progressive),
    /// matching the default used by [`BreakerLayer`](crate::breaker::BreakerLayer).
    fn default() -> Self {
        Self::progressive(None)
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(rename_all = "snake_case"))]
enum Mode {
    Quick,
    Progressive(
        #[cfg_attr(
            any(feature = "serde", test),
            serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::optional")
        )]
        Option<Duration>,
    ),
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[expect(clippy::float_cmp, reason = "simpler tests")]
mod tests {
    use super::*;
    use crate::breaker::engine::probing::ProbeOptions;

    #[test]
    fn quick_mode_creates_single_probe() {
        let mode = HalfOpenMode::quick();
        let options = mode.to_options(Duration::from_secs(30), 0.1, &AbandonedPolicy::default());
        let probes: Vec<_> = options.probes().collect();

        assert_eq!(probes.len(), 1);
        assert!(matches!(probes[0], ProbeOptions::SingleProbe { .. }));
    }

    #[test]
    fn quick_mode_uses_default_duration() {
        let mode = HalfOpenMode::quick();
        let default = Duration::from_secs(30);
        let options = mode.to_options(default, 0.1, &AbandonedPolicy::as_failures());
        let probes: Vec<_> = options.probes().collect();

        // The configured abandoned policy is threaded through to the single probe gate.
        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown, abandoned_policy }
                if *cooldown == default && *abandoned_policy == AbandonedPolicy::as_failures()
        ));
    }

    #[test]
    fn progressive_mode_creates_seven_probes() {
        let mode = HalfOpenMode::progressive(None);
        let options = mode.to_options(Duration::from_secs(30), 0.1, &AbandonedPolicy::default());

        assert_eq!(options.probes().len(), 7);
    }

    #[test]
    fn progressive_mode_with_custom_duration() {
        let custom = Duration::from_secs(45);
        let mode = HalfOpenMode::progressive(custom);
        let options = mode.to_options(Duration::from_secs(30), 0.1, &AbandonedPolicy::default());
        let probes: Vec<_> = options.probes().collect();

        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown, .. } if *cooldown == custom
        ));

        for probe in &probes[1..] {
            if let ProbeOptions::HealthProbe(h) = probe {
                assert_eq!(h.stage_duration(), custom);

                assert_eq!(h.failure_threshold(), 0.1);
            }
        }
    }

    #[test]
    fn progressive_mode_with_default_duration() {
        let mode = HalfOpenMode::progressive(None);
        let default = Duration::from_mins(1);
        let options = mode.to_options(default, 0.1, &AbandonedPolicy::default());
        let probes: Vec<_> = options.probes().collect();

        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown, .. } if *cooldown == default
        ));

        for probe in &probes[1..] {
            if let ProbeOptions::HealthProbe(h) = probe {
                assert_eq!(h.stage_duration(), default);
            }
        }
    }

    #[test]
    fn progressive_mode_clamps_min_duration() {
        let mode = HalfOpenMode::progressive(Duration::from_millis(500));

        assert!(matches!(mode.inner, Mode::Progressive(duration) if duration == Some(Duration::from_secs(1))));
    }
}
