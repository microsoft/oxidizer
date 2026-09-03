// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A portable compression-effort level from [`Level::MIN`] to [`Level::MAX`].
///
/// This is a newtype rather than a re-export of the underlying compression engine's level type.
/// Exposing the engine's type would make the engine part of this crate's semver surface, so
/// swapping or upgrading it would become a breaking change for every consumer.
///
/// The scale orders settings from lower effort and latency to higher effort and usually better
/// compression. It does not promise that zero disables compression or that nine is the strongest
/// setting a format supports; use a format-specific level type when exact native control matters.
///
/// # How the scale is calibrated
///
/// The `0..=9` range and the position of [`DEFAULT`][Self::DEFAULT] come from the deflate family,
/// whose native scale this is; every other format is mapped onto it rather than the other way
/// round. The anchor is meaning rather than arithmetic: each format's mapping is chosen so that
/// [`DEFAULT`][Self::DEFAULT] lands on that format's own balanced setting -- zstd's native 3, for
/// instance -- rather than on the midpoint of its native range. Where a format's own range climbs
/// steeply at the top, the mapping stops short of it instead of stretching to reach it, which is
/// why the top of this scale is not necessarily the top of a format's.
///
/// The scale is portable but its *cost* is not, and the difference between formats is large. On
/// the deflate family and on zstd, moving up the scale changes the time taken but barely moves the
/// memory used. On brotli both climb steeply towards the top of the range, while the ratio gained
/// over the middle of the range stays small. Treat [`Level::HIGH`] as a deliberate choice to be
/// measured on real payloads, not as a free improvement.
///
/// # Examples
///
/// ```
/// # #[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
/// # {
/// use compressors::Level;
///
/// assert_eq!(Level::default(), Level::DEFAULT);
/// assert_eq!(Level::new(9), Some(Level::HIGH));
/// assert_eq!(Level::new(10), None);
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(u8);

impl Level {
    /// The lowest-effort setting on the portable scale.
    pub const MIN: Self = Self(0);

    /// The fastest level that still compresses.
    pub const FAST: Self = Self(1);

    /// A balanced trade-off between speed and compression ratio.
    pub const DEFAULT: Self = Self(6);

    /// A high-compression setting at a correspondingly high cost.
    ///
    /// See the note on [`Level`] before reaching for this.
    pub const HIGH: Self = Self(9);

    /// The top of the portable scale, the same as [`Level::HIGH`].
    ///
    /// This is a `Level` rather than a bare number, so it can be passed straight to a builder.
    pub const MAX: Self = Self::HIGH;

    /// Creates a level, or returns `None` if `level` exceeds [`Level::MAX`].
    ///
    /// This returns an `Option` rather than panicking because levels routinely arrive from
    /// configuration files and command-line arguments, where an out-of-range value is a user
    /// mistake to be reported rather than a bug to crash on. Use [`TryFrom`] when you want that
    /// mistake as an [`Error`][crate::Error] to propagate with `?`.
    #[must_use]
    pub const fn new(level: u8) -> Option<Self> {
        if level > Self::MAX.0 { None } else { Some(Self(level)) }
    }

    /// Returns the level as a number in `0..=9`.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Level {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for Level {
    type Error = crate::Error;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        Self::new(level).ok_or_else(|| {
            crate::Error::invalid_configuration(format!(
                "compression level {level} is out of range; expected {}..={}",
                Self::MIN.0,
                Self::MAX.0
            ))
        })
    }
}

impl From<Level> for u8 {
    fn from(level: Level) -> Self {
        level.get()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_the_whole_valid_range() {
        for level in 0..=Level::MAX.get() {
            let parsed = Level::new(level).expect("level is within range");
            assert_eq!(parsed.get(), level);
        }
    }

    #[test]
    fn new_rejects_out_of_range_without_panicking() {
        assert_eq!(Level::new(10), None);
        assert_eq!(Level::new(u8::MAX), None);
    }

    #[test]
    fn bounds_are_levels_so_they_can_be_passed_to_a_builder() {
        assert_eq!(Level::MIN.get(), 0);
        assert_eq!(Level::MAX, Level::HIGH);
    }

    #[test]
    fn conversions_follow_the_standard_traits() {
        assert_eq!(Level::try_from(9).expect("in range"), Level::HIGH);
        assert_eq!(u8::from(Level::HIGH), 9);

        let error = Level::try_from(10).expect_err("out of range");
        assert!(error.is_invalid_configuration(), "got {error}");
        assert!(error.to_string().contains("0..=9"), "the message should name the range: {error}");
    }

    #[test]
    fn named_levels_have_the_expected_values() {
        assert_eq!(Level::MIN.get(), 0);
        assert_eq!(Level::FAST.get(), 1);
        assert_eq!(Level::DEFAULT.get(), 6);
        assert_eq!(Level::HIGH.get(), 9);
    }

    #[test]
    fn default_matches_the_default_constant() {
        assert_eq!(Level::default(), Level::DEFAULT);
    }

    #[test]
    fn levels_order_by_strength() {
        assert!(Level::MIN < Level::FAST);
        assert!(Level::FAST < Level::DEFAULT);
        assert!(Level::DEFAULT < Level::HIGH);
    }
}
