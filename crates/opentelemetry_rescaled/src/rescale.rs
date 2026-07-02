// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Value rescaling: applying a multiplicative factor to a measurement, with
//! type-appropriate rounding and saturation for integer instruments.

/// A measurement value that can be multiplied by an `f64` rescale factor.
///
/// Floating-point values multiply directly. Integer values multiply in `f64`,
/// round to the nearest integer, and saturate at the type's bounds rather than
/// wrapping, so a runaway sidecar stays bounded and obvious.
pub(crate) trait Rescale: Copy + Send + Sync + 'static {
    /// Returns `self` multiplied by `factor`, rounded and saturated as needed.
    fn rescale(self, factor: f64) -> Self;
}

impl Rescale for f64 {
    fn rescale(self, factor: f64) -> Self {
        self * factor
    }
}

impl Rescale for u64 {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "saturating round is the intended behavior; float->int `as` casts saturate at the type bounds and map NaN to 0"
    )]
    fn rescale(self, factor: f64) -> Self {
        (self as f64 * factor).round() as Self
    }
}

impl Rescale for i64 {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "saturating round is the intended behavior; float->int `as` casts saturate at the type bounds and map NaN to 0"
    )]
    fn rescale(self, factor: f64) -> Self {
        (self as f64 * factor).round() as Self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn f64_multiplies_directly() {
        assert!((1.5_f64.rescale(1000.0) - 1500.0).abs() < f64::EPSILON);
        assert!((2.0_f64.rescale(0.001) - 0.002).abs() < f64::EPSILON);
    }

    #[test]
    fn u64_rounds_to_nearest() {
        assert_eq!(1_u64.rescale(1000.0), 1000);
        // 3 * 0.5 = 1.5 -> rounds to 2 (round half away from zero).
        assert_eq!(3_u64.rescale(0.5), 2);
        // 2 * 0.5 = 1.0 -> exactly 1.
        assert_eq!(2_u64.rescale(0.5), 1);
        // 1 * 0.4 = 0.4 -> rounds to 0.
        assert_eq!(1_u64.rescale(0.4), 0);
    }

    #[test]
    fn u64_saturates_on_overflow() {
        assert_eq!(u64::MAX.rescale(1000.0), u64::MAX);
    }

    #[test]
    fn i64_rounds_and_handles_sign() {
        assert_eq!((-2_i64).rescale(1000.0), -2000);
        assert_eq!(3_i64.rescale(0.5), 2);
        assert_eq!((-3_i64).rescale(0.5), -2);
    }

    #[test]
    fn i64_saturates_at_both_bounds() {
        assert_eq!(i64::MAX.rescale(1000.0), i64::MAX);
        assert_eq!(i64::MIN.rescale(1000.0), i64::MIN);
    }
}
