// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::breaker::constants::MIN_SAMPLING_DURATION;
use crate::breaker::engine::probing::ProbesOptions;

/// The behavior of the circuit breaker when transitioning from half-open to closed state.
///
/// The half-open state is a transitional phase where the circuit breaker allows a limited number of
/// inputs to pass through to test if the underlying service has recovered. The chosen mode
/// determines how aggressively the circuit breaker probes the service during this phase.
///
/// Currently, two modes are supported:
///
/// - [`HalfOpenMode::quick`]: Allows a single probe to determine if the service has recovered.
/// - [`HalfOpenMode::progressive`]: Gradually increases the percentage of probes over multiple stages (default).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(feature = "serde", test), serde(transparent))]
pub struct HalfOpenMode {
    inner: Mode,
}

impl HalfOpenMode {
    /// Allow quick recovery from half-open state with a single probe.
    ///
    /// This approach is less reliable compared to the [`HalfOpenMode::progressive`] mode, but
    /// can close the circuit faster.
    ///
    /// The downside of this approach is that it relies on a single execution to determine
    /// the health of the service. If that execution happens to succeed by chance, the circuit
    /// closes and later inputs may fail again, leading to instability and re-opening the circuit
    /// again.
    #[must_use]
    pub fn quick() -> Self {
        Self { inner: Mode::Quick }
    }

    /// Gradually increase the percentage of probes over multiple stages.
    ///
    /// This approach allows more inputs to pass through in a controlled manner,
    /// increasing the probing rate over time. This can help more reliably evaluate the
    /// health of the underlying service over time rather than relying on a single execution.
    ///
    /// The pre-configured ratios for each probing stage are:
    /// `0.1%, 1%, 5%, 10%, 25%, 50%`
    ///
    /// Each probing stage advances after the stage duration has elapsed, and the health
    /// metrics indicate that the failure rate is below the configured threshold. If any probing stage
    /// fails, the circuit reopens immediately and the cycle starts over.
    ///
    /// The optional `stage_duration` specifies how long each probing stage lasts. If not
    /// provided, the value of [`break_duration`][crate::breaker::BreakerLayer::break_duration]
    /// is used. The provided stage duration is clamped to a minimum of 1 second.
    #[must_use]
    pub fn progressive(stage_duration: impl Into<Option<Duration>>) -> Self {
        Self {
            inner: Mode::Progressive(stage_duration.into().map(|d| d.max(MIN_SAMPLING_DURATION))),
        }
    }

    pub(super) fn to_options(&self, default_stage_duration: Duration, failure_threshold: f32) -> ProbesOptions {
        match self.inner {
            Mode::Quick => ProbesOptions::quick(default_stage_duration),
            Mode::Progressive(duration) => ProbesOptions::progressive(duration.unwrap_or(default_stage_duration), failure_threshold),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
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
        let options = mode.to_options(Duration::from_secs(30), 0.1);
        let probes: Vec<_> = options.probes().collect();

        assert_eq!(probes.len(), 1);
        assert!(matches!(probes[0], ProbeOptions::SingleProbe { .. }));
    }

    #[test]
    fn quick_mode_uses_default_duration() {
        let mode = HalfOpenMode::quick();
        let default = Duration::from_secs(30);
        let options = mode.to_options(default, 0.1);
        let probes: Vec<_> = options.probes().collect();

        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown } if *cooldown == default
        ));
    }

    #[test]
    fn progressive_mode_creates_seven_probes() {
        let mode = HalfOpenMode::progressive(None);
        let options = mode.to_options(Duration::from_secs(30), 0.1);

        assert_eq!(options.probes().len(), 7);
    }

    #[test]
    fn progressive_mode_with_custom_duration() {
        let custom = Duration::from_secs(45);
        let mode = HalfOpenMode::progressive(custom);
        let options = mode.to_options(Duration::from_secs(30), 0.1);
        let probes: Vec<_> = options.probes().collect();

        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown } if *cooldown == custom
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
        let default = Duration::from_secs(60);
        let options = mode.to_options(default, 0.1);
        let probes: Vec<_> = options.probes().collect();

        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown } if *cooldown == default
        ));

        for probe in &probes[1..] {
            if let ProbeOptions::HealthProbe(h) = probe {
                assert_eq!(h.stage_duration(), default);
            }
        }
    }

    #[test]
    fn progressive_mode_accepts_various_inputs() {
        let mode1 = HalfOpenMode::progressive(Duration::from_secs(10));
        let mode2 = HalfOpenMode::progressive(Some(Duration::from_secs(10)));
        let mode3 = HalfOpenMode::progressive(None);

        assert!(matches!(mode1.inner, Mode::Progressive(Some(_))));
        assert!(matches!(mode2.inner, Mode::Progressive(Some(_))));
        assert!(matches!(mode3.inner, Mode::Progressive(None)));
    }

    #[test]
    fn progressive_mode_clamps_min_duration() {
        let mode = HalfOpenMode::progressive(Duration::from_millis(500));

        assert!(matches!(mode.inner, Mode::Progressive(duration) if duration == Some(Duration::from_secs(1))));
    }

    #[test]
    fn serde_quick_roundtrip() {
        let mode = HalfOpenMode::quick();
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""Quick""#);

        let deserialized: HalfOpenMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, mode);
    }

    #[test]
    fn serde_progressive_no_duration_roundtrip() {
        let mode = HalfOpenMode::progressive(None);
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#"{"Progressive":null}"#);

        let deserialized: HalfOpenMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, mode);
    }

    #[test]
    fn serde_progressive_with_duration_roundtrip() {
        let mode = HalfOpenMode::progressive(Duration::from_secs(605));
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#"{"Progressive":"10m 5s"}"#);

        let deserialized: HalfOpenMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, mode);
    }

    #[test]
    fn serde_progressive_with_short_duration() {
        let mode = HalfOpenMode::progressive(Duration::from_secs(5));
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#"{"Progressive":"5s"}"#);

        let deserialized: HalfOpenMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, mode);
    }

    #[test]
    fn serde_deserialize_verbose_duration() {
        let deserialized: HalfOpenMode = serde_json::from_str(r#"{"Progressive":"1 hour, 30 minutes"}"#).unwrap();
        assert_eq!(deserialized, HalfOpenMode::progressive(Duration::from_secs(5400)));
    }

    #[test]
    fn serde_deserialize_invalid_variant() {
        let err = serde_json::from_str::<HalfOpenMode>(r#""Unknown""#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown variant"), "unexpected error: {msg}");
    }

    #[test]
    fn serde_deserialize_invalid_duration() {
        let err = serde_json::from_str::<HalfOpenMode>(r#"{"Progressive":"not_a_duration"}"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("friendly"), "expected jiff parse error, got: {msg}");
    }
}
