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
/// - [`HalfOpenMode::reliable`]: Gradually increases the percentage of probes over multiple stages (default).
#[derive(Debug, Clone, PartialEq)]
pub struct HalfOpenMode {
    inner: Mode,
}

impl HalfOpenMode {
    /// Allow quick recovery from half-open state with a single probe.
    ///
    /// This approach is less reliable compared to the [`HalfOpenMode::reliable`] mode, but
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
    pub fn reliable(stage_duration: impl Into<Option<Duration>>) -> Self {
        Self {
            inner: Mode::Reliable(stage_duration.into().map(|d| d.max(MIN_SAMPLING_DURATION))),
        }
    }

    pub(super) fn to_options(&self, default_stage_duration: Duration, failure_threshold: f32) -> ProbesOptions {
        match self.inner {
            Mode::Quick => ProbesOptions::quick(default_stage_duration),
            Mode::Reliable(duration) => ProbesOptions::reliable(duration.unwrap_or(default_stage_duration), failure_threshold),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Quick,
    Reliable(Option<Duration>),
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
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
    fn reliable_mode_creates_seven_probes() {
        let mode = HalfOpenMode::reliable(None);
        let options = mode.to_options(Duration::from_secs(30), 0.1);

        assert_eq!(options.probes().len(), 7);
    }

    #[test]
    fn reliable_mode_with_custom_duration() {
        let custom = Duration::from_secs(45);
        let mode = HalfOpenMode::reliable(custom);
        let options = mode.to_options(Duration::from_secs(30), 0.1);
        let probes: Vec<_> = options.probes().collect();

        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown } if *cooldown == custom
        ));

        #[expect(clippy::float_cmp, reason = "Test")]
        for probe in &probes[1..] {
            if let ProbeOptions::HealthProbe(h) = probe {
                assert_eq!(h.stage_duration(), custom);

                assert_eq!(h.failure_threshold(), 0.1);
            }
        }
    }

    #[test]
    fn reliable_mode_with_default_duration() {
        let mode = HalfOpenMode::reliable(None);
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
    fn reliable_mode_accepts_various_inputs() {
        let mode1 = HalfOpenMode::reliable(Duration::from_secs(10));
        let mode2 = HalfOpenMode::reliable(Some(Duration::from_secs(10)));
        let mode3 = HalfOpenMode::reliable(None);

        assert!(matches!(mode1.inner, Mode::Reliable(Some(_))));
        assert!(matches!(mode2.inner, Mode::Reliable(Some(_))));
        assert!(matches!(mode3.inner, Mode::Reliable(None)));
    }

    #[test]
    fn reliable_mode_clamps_min_duration() {
        let mode = HalfOpenMode::reliable(Duration::from_millis(500));

        assert!(matches!(mode.inner, Mode::Reliable(duration) if duration == Some(Duration::from_secs(1))));
    }
}
