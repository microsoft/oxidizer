// Copyright (c) Microsoft Corporation.

use std::cmp::min;
use std::time::Duration;

use crate::Backoff;
use crate::retry::constants::{DEFAULT_BACKOFF, DEFAULT_BASE_DELAY, DEFAULT_USE_JITTER};
use crate::rnd::Rnd;

/// The factor used to determine the range of jitter applied to delays.
const JITTER_FACTOR: f64 = 0.5;

/// The default factor used for exponential backoff calculations for cases where jitter is not applied.
const EXPONENTIAL_FACTOR: f64 = 2.0;

// The delay generation follows the Polly V8 implementation:
//
// https://github.com/App-vNext/Polly/blob/452b34ee1e3a45ccce156a6980f60901a623ee67/src/Polly.Core/Retry/RetryHelper.cs#L3
#[derive(Debug)]
pub(crate) struct DelayBackoff(pub(super) BackoffOptions);

impl From<BackoffOptions> for DelayBackoff {
    fn from(props: BackoffOptions) -> Self {
        Self(props)
    }
}

impl DelayBackoff {
    pub fn delays(&self) -> impl Iterator<Item = Duration> {
        DelaysIter {
            props: self.0.clone(),
            attempt: 0,
            prev: 0.0,
        }
    }
}

#[derive(Debug)]
struct DelaysIter {
    props: BackoffOptions,
    attempt: u32,
    // The state that is required to compute the next delay when using
    // decorrelated jitter backoff.
    prev: f64,
}

impl Iterator for DelaysIter {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        // zero base delay => always zero
        if self.props.base_delay.is_zero() {
            return Some(Duration::ZERO);
        }

        let next_attempt = self.attempt.saturating_add(1);
        let delay = match (self.props.backoff_type, self.props.use_jitter) {
            (Backoff::Constant, false) => self.props.base_delay,
            (Backoff::Constant, true) => apply_jitter(self.props.base_delay, &self.props.rnd),
            (Backoff::Linear, _) => {
                let delay = self.props.base_delay.saturating_mul(next_attempt);
                if self.props.use_jitter {
                    apply_jitter(delay, &self.props.rnd)
                } else {
                    delay
                }
            }
            (Backoff::Exponential, false) => duration_mul_pow2(self.props.base_delay, self.attempt),
            (Backoff::Exponential, true) => {
                decorrelated_jitter_backoff_v2(self.attempt, self.props.base_delay, &mut self.prev, &self.props.rnd)
            }
        };

        self.attempt = next_attempt;
        Some(clamp_to_max(delay, self.props.max_delay))
    }
}

fn clamp_to_max(d: Duration, max: Option<Duration>) -> Duration {
    max.map_or(d, |m| min(d, m))
}

fn duration_mul_pow2(base: Duration, attempt: u32) -> Duration {
    let factor = EXPONENTIAL_FACTOR.powi(i32::try_from(attempt).unwrap_or(i32::MAX));
    secs_to_duration_saturating(base.as_secs_f64() * factor)
}

/// Adds a symmetric, uniform jitter around the given delay.
///
/// - Jitter is in both directions and relative to `delay` (centered on it).
/// - With `JITTER_FACTOR = 0.5`, the result lies in `[0.75*delay, 1.25*delay]`.
/// - Randomness comes from [`Rnd`]; conversion saturates on overflow and clamps at zero.
#[inline]
fn apply_jitter(delay: Duration, rnd: &Rnd) -> Duration {
    let ms = delay.as_secs_f64() * 1000.0;
    let offset = (ms * JITTER_FACTOR) / 2.0;
    let random_delay = (ms * JITTER_FACTOR).mul_add(rnd.next_f64(), -offset);
    let new_ms = ms + random_delay;

    secs_to_duration_saturating(new_ms / 1000.0)
}

/// De-correlated jitter backoff (`v2`): smooth exponential growth with bounded randomization.
///
/// De-correlated jitter `V2` spreads retries evenly while preserving exponential backoff
/// (with a configurable first-retry median), reducing synchronized spikes and tail-latency
/// compared to naive random jitter.
///
/// What does "de-correlated" mean?
///
/// - Successive delays are not a direct function of the immediately previous
///   delay. Instead, each step samples a random phase (`t = attempt + U[0,1)`)
///   and advances a smooth curve; we only take the delta from the previous
///   position on that curve. This weakens correlation between consecutive
///   samples and reduces synchronization across many callers.
///
/// What does `v2` mean?
///
/// - It refers to the second-generation formulation from
///   `Polly.Contrib.WaitAndRetry` (linked below). Compared to the earlier
///   (`v1`) "de-correlated jitter" popularized in the `AWS` blog post, `v2` uses a
///   closed-form function combining exponential growth with a `tanh(sqrt(p*t))`
///   taper to achieve monotonic expected growth, reduced tail latency, and a
///   tighter distribution while still remaining de-correlated.
///
/// References
/// - [`Polly V8` implementation](https://github.com/App-vNext/Polly/blob/8ba1e3ba295542cbc937d0555fadfa0d23b5c568/src/Polly.Core/Retry/RetryHelper.cs#L96)
/// - [`Polly V7` implementation](https://github.com/Polly-Contrib/Polly.Contrib.WaitAndRetry/blob/7596d2dacf22d88bbd814bc49c28424fb6e921e9/src/Polly.Contrib.WaitAndRetry/Backoff.DecorrelatedJitterV2.cs#L22)
/// - [`Polly.Contrib.WaitAndRetry` repo](https://github.com/Polly-Contrib/Polly.Contrib.WaitAndRetry)
#[inline]
fn decorrelated_jitter_backoff_v2(attempt: u32, base_delay: Duration, prev: &mut f64, rnd: &Rnd) -> Duration {
    // The original author/credit for this jitter formula is @george-polevoy .
    // Jitter formula used with permission as described at https://github.com/App-vNext/Polly/issues/530#issuecomment-526555979
    // Minor adaptations (pFactor = 4.0 and rpScalingFactor = 1 / 1.4d) by @reisenberger, to scale the formula output for easier parameterization to users.

    // A factor used within the formula to help smooth the first calculated delay.
    const P_FACTOR: f64 = 4.0;

    // A factor used to scale the median values of the retry times generated by the formula to be _near_ whole seconds, to aid Polly user comprehension.
    // This factor allows the median values to fall approximately at 1, 2, 4 etc seconds, instead of 1.4, 2.8, 5.6, 11.2.
    const RP_SCALING: f64 = 1.0 / 1.4;

    let target_secs_first_delay = base_delay.as_secs_f64();

    let t = f64::from(attempt) + rnd.next_f64();
    let next = t.exp2() * (P_FACTOR * t).sqrt().tanh();

    if !next.is_finite() {
        *prev = next;
        return Duration::MAX;
    }

    let formula_intrinsic_value = next - *prev;
    *prev = next;

    secs_to_duration_saturating(formula_intrinsic_value * RP_SCALING * target_secs_first_delay)
}

fn secs_to_duration_saturating(secs: f64) -> Duration {
    if secs <= 0.0 {
        return Duration::ZERO;
    }

    Duration::try_from_secs_f64(secs).unwrap_or(Duration::MAX)
}

#[derive(Debug, Clone)]
pub(super) struct BackoffOptions {
    pub backoff_type: Backoff,
    pub base_delay: Duration,
    pub max_delay: Option<Duration>,
    pub use_jitter: bool,
    pub rnd: Rnd,
}

impl Default for BackoffOptions {
    fn default() -> Self {
        Self {
            backoff_type: DEFAULT_BACKOFF,
            base_delay: DEFAULT_BASE_DELAY,
            max_delay: None,
            use_jitter: DEFAULT_USE_JITTER,
            rnd: Rnd::default(),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn default_props() {
        let props = BackoffOptions::default();

        assert_eq!(props.backoff_type, Backoff::Exponential);
        assert_eq!(props.base_delay, Duration::from_secs(2));
        assert_eq!(props.max_delay, None);
        assert!(props.use_jitter);
    }

    #[test]
    fn smoke_constant_no_jitter() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Constant,
            base_delay: Duration::from_millis(200),
            max_delay: None,
            use_jitter: false,
            rnd: Rnd::default(),
        });
        let v: Vec<_> = backoff.delays().take(3).collect();
        assert_eq!(v, vec![Duration::from_millis(200); 3]);
    }

    #[test]
    fn smoke_linear_no_jitter() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Linear,
            base_delay: Duration::from_millis(100),
            max_delay: None,
            use_jitter: false,
            rnd: Rnd::default(),
        });

        let v: Vec<_> = backoff.delays().take(4).collect();
        assert_eq!(
            v,
            vec![
                Duration::from_millis(100),
                Duration::from_millis(200),
                Duration::from_millis(300),
                Duration::from_millis(400),
            ]
        );
    }

    #[test]
    fn smoke_exponential_cap() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(100),
            max_delay: Some(Duration::from_secs(1)),
            use_jitter: false,
            rnd: Rnd::default(),
        });

        // 100ms, 200ms, 400ms, 800ms, then clamped at 1s
        let v: Vec<_> = backoff.delays().take(6).collect();
        assert_eq!(v[0], Duration::from_millis(100));
        assert_eq!(v[1], Duration::from_millis(200));
        assert_eq!(v[2], Duration::from_millis(400));
        assert_eq!(v[3], Duration::from_millis(800));
        assert_eq!(v[4], Duration::from_secs(1));
        assert_eq!(v[5], Duration::from_secs(1));
    }

    #[test]
    fn zero_base_delay_always_zero() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::ZERO,
            max_delay: None,
            use_jitter: true,
            rnd: Rnd::default(),
        });
        let v: Vec<_> = backoff.delays().take(5).collect();
        assert!(v.iter().all(|d| *d == Duration::ZERO));
    }

    #[test]
    fn constant_with_jitter() {
        // Test with fixed random values to verify jitter behavior
        let rnd = Rnd::new_fixed(0.0);

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Constant,
            base_delay: Duration::from_secs(1),
            max_delay: None,
            use_jitter: true,
            rnd,
        });

        let v: Vec<_> = backoff.delays().take(3).collect();
        // With random value 0.0, jitter should give us 0.75 seconds
        assert_eq!(v[0], Duration::from_millis(750));
        assert_eq!(v[1], Duration::from_millis(750));
        assert_eq!(v[2], Duration::from_millis(750));
    }

    #[test]
    fn constant_with_different_jitter_values() {
        // Test with random value 0.4
        let rnd = Rnd::new_fixed(0.4);

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Constant,
            base_delay: Duration::from_secs(1),
            max_delay: None,
            use_jitter: true,
            rnd,
        });

        let delay = backoff.delays().next().unwrap();
        // With random value 0.4, jitter should give us 0.95 seconds
        assert_eq!(delay, Duration::from_millis(950));

        // Test with random value 1.0
        let rnd = Rnd::new_fixed(1.0);

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Constant,
            base_delay: Duration::from_secs(1),
            max_delay: None,
            use_jitter: true,
            rnd,
        });

        let delay = backoff.delays().next().unwrap();
        // With random value 1.0, jitter should give us 1.25 seconds
        assert_eq!(delay, Duration::from_millis(1250));
    }

    #[test]
    fn linear_with_jitter() {
        // Test with fixed random value 0.5
        let rnd = Rnd::new_fixed(0.5);

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Linear,
            base_delay: Duration::from_secs(1),
            max_delay: None,
            use_jitter: true,
            rnd,
        });

        let v: Vec<_> = backoff.delays().take(3).collect();
        // attempt 0: base_delay * 1 = 1s, with jitter 0.5 should be exactly 1s
        // attempt 1: base_delay * 2 = 2s, with jitter 0.5 should be exactly 2s
        // attempt 2: base_delay * 3 = 3s, with jitter 0.5 should be exactly 3s
        assert_eq!(v[0], Duration::from_secs(1));
        assert_eq!(v[1], Duration::from_secs(2));
        assert_eq!(v[2], Duration::from_secs(3));
    }

    #[test]
    fn linear_with_different_jitter_values() {
        // Test linear with various jitter values for attempt 2 (3rd delay)
        let test_cases = [
            (0.0, 2250), // 3s * (1 + 0.5 * (0.0 - 0.5)) = 2.25s
            (0.4, 2850), // 3s * (1 + 0.5 * (0.4 - 0.5)) = 2.85s
            (0.6, 3150), // 3s * (1 + 0.5 * (0.6 - 0.5)) = 3.15s
            (1.0, 3750), // 3s * (1 + 0.5 * (1.0 - 0.5)) = 3.75s
        ];

        for (random_val, expected_ms) in test_cases {
            let rnd = Rnd::new_fixed(random_val);

            let backoff = DelayBackoff(BackoffOptions {
                backoff_type: Backoff::Linear,
                base_delay: Duration::from_secs(1),
                max_delay: None,
                use_jitter: true,
                rnd,
            });

            let v: Vec<_> = backoff.delays().take(3).collect();
            assert_eq!(v[2], Duration::from_millis(expected_ms));
        }
    }

    #[test]
    fn exponential_no_jitter() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_secs(1),
            max_delay: None,
            use_jitter: false,
            rnd: Rnd::default(),
        });

        let v: Vec<_> = backoff.delays().take(3).collect();
        assert_eq!(
            v,
            vec![
                Duration::from_secs(1), // 2^0 = 1
                Duration::from_secs(2), // 2^1 = 2
                Duration::from_secs(4), // 2^2 = 4
            ]
        );
    }

    #[test]
    fn max_delay_respected_all_types() {
        let max_delay = Duration::from_secs(1);

        // Test constant with large base delay
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Constant,
            base_delay: Duration::from_secs(10),
            max_delay: Some(max_delay),
            use_jitter: false,
            rnd: Rnd::default(),
        });
        let v: Vec<_> = backoff.delays().take(3).collect();
        assert!(v.iter().all(|d| *d == max_delay));

        // Test linear with large multiplier
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Linear,
            base_delay: Duration::from_secs(10),
            max_delay: Some(max_delay),
            use_jitter: false,
            rnd: Rnd::default(),
        });
        let v: Vec<_> = backoff.delays().take(3).collect();
        assert!(v.iter().all(|d| *d == max_delay));

        // Test exponential with large base delay
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_secs(10),
            max_delay: Some(max_delay),
            use_jitter: false,
            rnd: Rnd::default(),
        });
        let v: Vec<_> = backoff.delays().take(3).collect();
        assert!(v.iter().all(|d| *d == max_delay));
    }

    #[test]
    fn max_delay_with_jitter() {
        let rnd = Rnd::new_fixed(0.5);

        let max_delay = Duration::from_secs(1);

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Linear,
            base_delay: Duration::from_secs(10),
            max_delay: Some(max_delay),
            use_jitter: true,
            rnd,
        });

        let v: Vec<_> = backoff.delays().take(3).collect();
        assert!(v.iter().all(|d| *d == max_delay));
    }

    #[test]
    fn delay_less_than_max_delay_respected() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Constant,
            base_delay: Duration::from_secs(1),
            max_delay: Some(Duration::from_secs(2)),
            use_jitter: false,
            rnd: Rnd::default(),
        });

        let v: Vec<_> = backoff.delays().take(3).collect();
        assert!(v.iter().all(|d| *d == Duration::from_secs(1)));
    }

    #[test]
    fn exponential_overflow_returns_max_duration() {
        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_secs(86400), // 1 day
            max_delay: None,
            use_jitter: false,
            rnd: Rnd::default(),
        });

        // Large attempt should cause overflow and return Duration::MAX
        let v: Vec<_> = backoff.delays().skip(1000).take(1).collect();
        assert_eq!(v[0], Duration::MAX);
    }

    #[test]
    fn exponential_overflow_with_max_delay() {
        let max_delay = Duration::from_secs(172_800); // 2 days

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_secs(86400), // 1 day
            max_delay: Some(max_delay),
            use_jitter: false,
            rnd: Rnd::default(),
        });

        // Large attempt should cause overflow but be clamped to max_delay
        let v: Vec<_> = backoff.delays().skip(1000).take(1).collect();
        assert_eq!(v[0], max_delay);
    }

    #[test]
    fn exponential_with_jitter_is_positive() {
        let test_attempts = [1, 2, 3, 4, 10, 100, 1000, 1024, 1025];

        for attempt in test_attempts {
            let backoff = DelayBackoff(BackoffOptions {
                backoff_type: Backoff::Exponential,
                base_delay: Duration::from_secs(2),
                max_delay: None,
                use_jitter: true,
                rnd: Rnd::default(),
            });

            let delays: Vec<_> = backoff.delays().skip(attempt).take(2).collect();
            assert!(delays[0] > Duration::ZERO, "Attempt {attempt}: first delay should be positive");
            assert!(delays[1] > Duration::ZERO, "Attempt {attempt}: second delay should be positive");
        }
    }

    #[test]
    fn exponential_with_jitter_respects_max_delay() {
        let test_attempts = [1, 2, 3, 4, 10, 100, 1000, 1024, 1025];
        let max_delay = Duration::from_secs(30);

        for attempt in test_attempts {
            let backoff = DelayBackoff(BackoffOptions {
                backoff_type: Backoff::Exponential,
                base_delay: Duration::from_secs(2),
                max_delay: Some(max_delay),
                use_jitter: true,
                rnd: Rnd::default(),
            });

            let delays: Vec<_> = backoff.delays().skip(attempt).take(2).collect();
            assert!(delays[0] > Duration::ZERO, "Attempt {attempt}: first delay should be positive");
            assert!(delays[0] <= max_delay, "Attempt {attempt}: first delay should not exceed max");
            assert!(delays[1] > Duration::ZERO, "Attempt {attempt}: second delay should be positive");
            assert!(delays[1] <= max_delay, "Attempt {attempt}: second delay should not exceed max");
        }
    }

    #[test]
    fn exponential_with_jitter_reproducible_with_fixed_values() {
        let rnd1 = Rnd::new_fixed(0.5);
        let backoff1 = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(7800), // 7.8 seconds
            max_delay: None,
            use_jitter: true,
            rnd: rnd1,
        });

        let rnd2 = Rnd::new_fixed(0.5);
        let backoff2 = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(7800), // 7.8 seconds
            max_delay: None,
            use_jitter: true,
            rnd: rnd2,
        });

        let delays1: Vec<_> = backoff1.delays().take(10).collect();
        let delays2: Vec<_> = backoff2.delays().take(10).collect();

        assert_eq!(delays1, delays2);
        assert!(delays1.iter().all(|d| *d > Duration::ZERO));
    }

    #[test]
    fn exponential_with_jitter_different_values_different_results() {
        let rnd1 = Rnd::new_fixed(0.2);
        let backoff1 = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(7800), // 7.8 seconds
            max_delay: None,
            use_jitter: true,
            rnd: rnd1,
        });

        let rnd2 = Rnd::new_fixed(0.8);
        let backoff2 = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(7800), // 7.8 seconds
            max_delay: None,
            use_jitter: true,
            rnd: rnd2,
        });

        let delays1: Vec<_> = backoff1.delays().take(10).collect();
        let delays2: Vec<_> = backoff2.delays().take(10).collect();

        assert_ne!(delays1, delays2);
        assert!(delays1.iter().all(|d| *d > Duration::ZERO));
        assert!(delays2.iter().all(|d| *d > Duration::ZERO));
    }

    // This test checks that the exponential backoff with jitter produces the same sequence of delays
    // as Polly v8:
    //
    // https://github.com/App-vNext/Polly/blob/452b34ee1e3a45ccce156a6980f60901a623ee67/test/Polly.Core.Tests/Retry/RetryHelperTests.cs#L254
    #[test]
    fn exponential_with_jitter_compatibility_with_polly_v8() {
        let random_values = Mutex::new(
            [
                0.726_243_269_967_959_8,
                0.817_325_359_590_968_7,
                0.768_022_689_394_663_4,
                0.558_161_191_436_537_2,
                0.206_033_154_021_032_7,
                0.558_884_794_618_415_1,
                0.906_027_066_011_925_7,
                0.442_177_873_310_715_84,
                0.977_549_753_141_379_8,
                0.273_704_457_689_870_34,
            ]
            .into_iter(),
        );

        let delays_ms = [8_626, 10_830, 18_396, 27_703, 37_213, 159_824, 405_539, 300_743, 1_839_611, 639_970];

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(7800), // 7.8 seconds
            max_delay: None,
            use_jitter: true,
            rnd: Rnd::new_function(move || random_values.lock().unwrap().next().unwrap()),
        });

        let computed: Vec<_> = backoff.delays().take(10).map(|v| v.as_millis()).collect();
        assert_eq!(computed, delays_ms);
    }

    #[test]
    fn exponential_without_jitter_ensure_expected_delays() {
        let random_values = Mutex::new(
            [
                0.726_243_269_967_959_8,
                0.817_325_359_590_968_7,
                0.768_022_689_394_663_4,
                0.558_161_191_436_537_2,
                0.206_033_154_021_032_7,
                0.558_884_794_618_415_1,
                0.906_027_066_011_925_7,
                0.442_177_873_310_715_84,
                0.977_549_753_141_379_8,
                0.273_704_457_689_870_34,
            ]
            .into_iter(),
        );

        let delays_ms = [7800, 15600, 31200, 62400, 124_800, 249_600, 499_200, 998_400, 1_996_800, 3_993_600];

        let backoff = DelayBackoff(BackoffOptions {
            backoff_type: Backoff::Exponential,
            base_delay: Duration::from_millis(7800), // 7.8 seconds
            max_delay: None,
            use_jitter: false,
            rnd: Rnd::new_function(move || random_values.lock().unwrap().next().unwrap()),
        });

        let computed: Vec<_> = backoff.delays().take(10).map(|v| v.as_millis()).collect();
        assert_eq!(computed, delays_ms);
    }
}
