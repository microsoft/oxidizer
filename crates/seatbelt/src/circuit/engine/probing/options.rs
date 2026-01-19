// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;
use std::vec::IntoIter;

use crate::circuit::HealthMetricsBuilder;

/// The minimum throughput during probing stage is set to 1, so at least one request must come
/// through in each probing stage to evaluate the health.
const MIN_THROUGHPUT: u32 = 1;

/// Options for a single probe type.
#[derive(Debug, Clone)]
pub(crate) enum ProbeOptions {
    /// A single probe that allows one probe.
    ///
    /// After the initial probe is allowed, it enters a cool-down period during which
    /// no further probes are allowed.
    SingleProbe { cooldown: Duration },

    /// A health-based probe that uses health metrics to determine the health of the system.
    HealthProbe(HealthProbeOptions),
}

/// Configuration options for the probing mechanism.
#[derive(Debug, Clone)]
pub(crate) struct ProbesOptions {
    probes: Vec<ProbeOptions>,
}

impl ProbesOptions {
    pub fn quick(cooldown: Duration) -> Self {
        Self::new([ProbeOptions::SingleProbe { cooldown }])
    }

    pub fn reliable(stage_duration: Duration, failure_threshold: f32) -> Self {
        Self::gradual(&[0.001, 0.01, 0.05, 0.1, 0.25, 0.5], stage_duration, failure_threshold)
    }

    pub fn gradual(probing_ratio: &[f64], stage_duration: Duration, failure_threshold: f32) -> Self {
        // Start with a single probe
        let initial = std::iter::once(ProbeOptions::SingleProbe { cooldown: stage_duration });

        // Then continue with health-based probes
        let health = probing_ratio
            .iter()
            .map(|probing_ratio| ProbeOptions::HealthProbe(HealthProbeOptions::new(stage_duration, failure_threshold, *probing_ratio)));

        Self::new(initial.chain(health))
    }

    pub fn new(probes: impl IntoIterator<Item = ProbeOptions>) -> Self {
        let probes: Vec<ProbeOptions> = probes.into_iter().collect();
        assert!(!probes.is_empty(), "the probes list cannot be empty");
        Self { probes }
    }

    pub fn probes(&self) -> IntoIter<ProbeOptions> {
        self.probes.clone().into_iter()
    }
}

#[derive(Debug, Clone)]
pub struct HealthProbeOptions {
    pub(super) builder: HealthMetricsBuilder,
    pub(super) probing_ratio: f64,
}

impl HealthProbeOptions {
    pub fn new(stage_duration: Duration, failure_threshold: f32, probing_ratio: f64) -> Self {
        assert!(probing_ratio > 0.0 && probing_ratio <= 1.0, "probing_ratio must be in (0.0, 1.0]");
        assert!((0.0..1.0).contains(&failure_threshold), "failure_threshold must be in [0.0, 1.0)");
        assert!(stage_duration > Duration::ZERO, "stage_duration must be greater than zero");

        Self {
            // The min throughput is set to 0, so if no requests come in during the probing stage,
            // the health will be considered healthy by default.
            builder: HealthMetricsBuilder::new(stage_duration, failure_threshold, MIN_THROUGHPUT),
            probing_ratio,
        }
    }

    pub fn stage_duration(&self) -> Duration {
        self.builder.sampling_duration
    }

    #[cfg(test)]
    pub fn failure_threshold(&self) -> f32 {
        self.builder.failure_threshold
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(ProbeOptions: Clone, std::fmt::Debug);
    assert_impl_all!(ProbesOptions: Clone, std::fmt::Debug);
    #[test]
    fn single_probe_constructor_creates_correct_options() {
        let cooldown = Duration::from_secs(15);
        let options = ProbesOptions::quick(cooldown);
        let probes: Vec<_> = options.probes().collect();

        assert_eq!(probes.len(), 1);
        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown: c } if *c == Duration::from_secs(15)
        ));
    }

    #[test]
    fn new_accepts_multiple_probes() {
        let options = ProbesOptions::new([
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(10),
            },
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(20),
            },
            ProbeOptions::SingleProbe {
                cooldown: Duration::from_secs(30),
            },
        ]);

        let probes: Vec<_> = options.probes().collect();
        assert_eq!(probes.len(), 3);
        assert!(matches!(&probes[0], ProbeOptions::SingleProbe { cooldown } if *cooldown == Duration::from_secs(10)));
        assert!(matches!(&probes[1], ProbeOptions::SingleProbe { cooldown } if *cooldown == Duration::from_secs(20)));
        assert!(matches!(&probes[2], ProbeOptions::SingleProbe { cooldown } if *cooldown == Duration::from_secs(30)));
    }

    #[test]
    fn clone_preserves_probe_count() {
        let options = ProbesOptions::quick(Duration::from_secs(25));
        let cloned = options.clone();

        assert_eq!(options.probes().count(), cloned.probes().count());
    }

    #[test]
    fn probes_iterator_is_reusable() {
        let options = ProbesOptions::quick(Duration::from_secs(30));

        assert_eq!(options.probes().count(), 1);
        assert_eq!(options.probes().count(), 1);
    }

    #[test]
    #[should_panic(expected = "the probes list cannot be empty")]
    fn new_panics_with_empty_iterator() {
        let _ = ProbesOptions::new(Vec::<ProbeOptions>::new());
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
    fn health_probe_options_ctor_ok() {
        let sampling_duration = Duration::from_secs(60);
        let failure_threshold = 0.2;
        let probing_ratio = 0.1;

        let options = HealthProbeOptions::new(sampling_duration, failure_threshold, probing_ratio);

        assert_eq!(options.stage_duration(), sampling_duration);
        assert_eq!(options.probing_ratio, probing_ratio);
        assert_eq!(options.builder.failure_threshold, failure_threshold);
        assert_eq!(options.builder.min_throughput, 1);
    }

    #[should_panic(expected = "stage_duration must be greater than zero")]
    #[test]
    fn health_probe_options_ctor_sampling_duration() {
        let _ = HealthProbeOptions::new(Duration::ZERO, 0.1, 0.5);
    }

    #[should_panic(expected = "failure_threshold must be in [0.0, 1.0)")]
    #[test]
    fn health_probe_options_ctor_failure_threshold() {
        let _ = HealthProbeOptions::new(Duration::from_secs(10), 1.0, 0.5);
    }

    #[should_panic(expected = "probing_ratio must be in (0.0, 1.0]")]
    #[test]
    fn health_probe_options_ctor_probing_ratio() {
        let _ = HealthProbeOptions::new(Duration::from_secs(10), 0.1, 0.0);
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
    fn probes_options_reliable_ok() {
        let options = ProbesOptions::reliable(Duration::from_secs(30), 0.2);
        let probes: Vec<_> = options.probes().collect();

        assert_eq!(probes.len(), 7);
        assert!(matches!(
            &probes[0],
            ProbeOptions::SingleProbe { cooldown } if *cooldown == Duration::from_secs(30)
        ));

        let expected_ratios = [0.001, 0.01, 0.05, 0.1, 0.25, 0.5];
        for (i, ratio) in expected_ratios.iter().enumerate() {
            let probe = &probes[i + 1];

            match probe {
                ProbeOptions::HealthProbe(options) => {
                    assert_eq!(options.builder.sampling_duration, Duration::from_secs(30));
                    assert_eq!(options.builder.failure_threshold, 0.2);
                    assert_eq!(options.probing_ratio, *ratio);
                }
                ProbeOptions::SingleProbe { .. } => panic!("expected HealthProbe"),
            }
        }
    }
}
