// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A portable compression-effort level from [`Level::MIN`] to [`Level::MAX`].
///
/// This newtype does not expose the compression engine level type. Therefore, an engine replacement
/// or upgrade does not change this public type.
///
/// The scale orders settings from lower effort and latency to higher effort and usually better
/// compression. It does not promise that zero disables compression or that nine is the strongest
/// setting that a format supports. Use a format-specific level type for exact native control.
///
/// # How the scale is calibrated
///
/// The `0..=9` range and the position of [`DEFAULT`][Self::DEFAULT] come from the deflate family,
/// whose native scale this is. Each other format maps onto this range. The mapping places
/// [`DEFAULT`][Self::DEFAULT] on the balanced setting for the format. For example, it maps to zstd
/// level 3. A mapping can stop below the highest native level when its cost rises steeply.
///
/// Cost differs by format and payload. Some backends also use more working memory at higher levels.
/// Moving up this scale does not lower effort. Measure [`Level::HIGH`] with representative payloads.
///
/// # Examples
///
/// ```
/// # #[cfg(any_format)]
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
    /// Measure this setting with representative payloads.
    pub const HIGH: Self = Self(9);

    /// The top of the portable scale, the same as [`Level::HIGH`].
    ///
    /// This is a `Level` rather than a bare number, so it can be passed straight to a builder.
    pub const MAX: Self = Self::HIGH;

    /// Creates a level, or returns `None` if `level` exceeds [`Level::MAX`].
    ///
    /// This returns an `Option` because levels can come from configuration files or command-line
    /// arguments. Use [`TryFrom`] to return an [`Error`][crate::Error] for an invalid value.
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
    #[inline]
    fn from(level: Level) -> Self {
        level.get()
    }
}
