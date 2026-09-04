// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Zstandard (RFC 8878): fast compression with ratios well beyond the deflate family.
//!
//! The format behind HTTP `Content-Encoding: zstd`, and the usual choice when both compression time
//! and ratio matter -- though where it wins over the alternatives depends on the payload, so
//! benchmark a representative corpus. Requires the `zstd` cargo feature.
//!
//! Unlike this crate's other formats, zstd is currently provided by a C library compiled from
//! bundled sources, so enabling it requires a C compiler. Builds that leave the feature off stay
//! pure Rust.
//!
//! # Examples
//!
//! ```
//! use compressors::{Resources, zstd};
//!
//! let compressed = zstd::compress(b"the quick brown fox", &Resources::default())?;
//! assert_eq!(
//!     compressed.range(0..4).to_vec(),
//!     vec![0x28, 0xb5, 0x2f, 0xfd]
//! );
//!
//! assert_eq!(
//!     zstd::decompress(compressed, &Resources::default())?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

mod codec;

use crate::limits::FormatLimits;
use crate::macros::define_format;
use crate::zstd::codec::{ZstdCompress, ZstdDecompress};

/// Zstd's default bounds.
///
/// Zstd has no structural expansion ceiling, so like brotli it needs a far looser ratio than the
/// deflate family. That ratio is a coarse backstop rather than real protection; what bounds
/// untrusted zstd is the cap the buffering conveniences apply, see
/// [`DecompressorLimits`][crate::DecompressorLimits].
///
/// The number itself is a policy choice, not a property of the format: any sufficiently
/// compressible input can legitimately expand by a very large factor, so no ratio both admits
/// legitimate data and excludes a bomb. The rule it encodes is "high enough that no realistic
/// payload trips it, low enough to still catch an obviously degenerate stream", and it is one
/// significant figure on purpose -- treat it as adjustable, not as a measured boundary. Anything of
/// the same order of magnitude would serve equally well.
pub(crate) const DEFAULT_LIMITS: FormatLimits = FormatLimits::new(Some(250_000), None, None);

/// Selects zstd as the format of a [`CompressorBuilder`][crate::CompressorBuilder] or [`DecompressorBuilder`][crate::DecompressorBuilder], and carries
/// the settings only zstd has.
///
/// Naming the format in the builder's type parameter is what gives that builder a `build` method
/// producing this module's [`Compressor`] and [`Decompressor`], along with the setters below.
#[derive(Debug, Clone)]
pub struct Zstd {
    level: Option<CompressionLevel>,
    max_window_log: Option<WindowLog>,
}

impl Zstd {
    /// The settings a zstd builder starts with: zstd's own defaults, and the portable
    /// [`Level`][crate::Level] left in charge of the compression level.
    pub(crate) const fn new() -> Self {
        Self {
            level: None,
            max_window_log: None,
        }
    }
}

define_format! {
    name = "zstd",
    format = Zstd,
    build_method = build_zstd,
    compressor_codec = ZstdCompress,
    compressor_build = fallible,
    new_compressor = ZstdCompress::new,
    decompressor_codec = ZstdDecompress,
    decompressor_build = fallible,
    default_limits = DEFAULT_LIMITS,
    new_decompressor = ZstdDecompress::new,
    multi_stream_default = true,
}

/// A level on zstd's own scale, for reaching settings the portable [`Level`][crate::Level] does not cover.
///
/// The portable scale is anchored on zstd's default so that [`Level::DEFAULT`][crate::Level::DEFAULT] means the same
/// thing on every format, and it maps onto a positive subset of zstd's range. This type exposes the
/// whole range the bundled library reports, including the negative fast levels below `1` and the
/// strong levels above the portable maximum.
///
/// Compression time rises steeply towards the strong end while the ratio gain flattens, and where
/// that trade stops being worthwhile depends on the payload and the hardware, so benchmark a
/// representative corpus rather than assuming a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompressionLevel(i32);

impl CompressionLevel {
    /// Zstd's own default, which the portable [`Level::DEFAULT`][crate::Level::DEFAULT] also maps to.
    pub const DEFAULT: Self = Self(3);

    /// Creates a level in the range supported by the bundled zstd library.
    #[must_use]
    pub fn new(level: i32) -> Option<Self> {
        if level < zstd_safe::min_c_level() || level > zstd_safe::max_c_level() {
            return None;
        }

        Some(Self(level))
    }

    /// The fastest level supported by the bundled zstd library.
    #[must_use]
    #[inline]
    pub fn min() -> Self {
        Self(zstd_safe::min_c_level())
    }

    /// The strongest level supported by the bundled zstd library.
    #[must_use]
    #[inline]
    pub fn max() -> Self {
        Self(zstd_safe::max_c_level())
    }

    /// Returns the level on zstd's scale.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl Default for CompressionLevel {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<i32> for CompressionLevel {
    type Error = crate::Error;

    fn try_from(level: i32) -> core::result::Result<Self, Self::Error> {
        Self::new(level).ok_or_else(|| {
            crate::Error::invalid_configuration(format!(
                "zstd compression level {level} is out of range; expected {}..={}",
                Self::min().0,
                Self::max().0
            ))
        })
    }
}

impl From<CompressionLevel> for i32 {
    #[inline]
    fn from(level: CompressionLevel) -> Self {
        level.get()
    }
}

/// The base-2 logarithm of the largest zstd window a decompressor will accept.
///
/// The default allows windows up to 128 MiB. Lowering it limits decompressor memory at the cost of
/// rejecting streams produced with larger windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowLog(u32);

impl WindowLog {
    /// The smallest zstd window, 1 KiB.
    pub const MIN: Self = Self(10);

    /// Zstd's default decompressor limit, 128 MiB.
    pub const DEFAULT: Self = Self(27);

    /// The largest supported window on this target.
    pub const MAX: Self = Self(if usize::BITS == 32 { 30 } else { 31 });

    /// Creates a window logarithm accepted by zstd on this target.
    #[must_use]
    pub const fn new(log: u32) -> Option<Self> {
        if log < Self::MIN.0 || log > Self::MAX.0 {
            None
        } else {
            Some(Self(log))
        }
    }

    /// Returns the base-2 logarithm.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for WindowLog {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u32> for WindowLog {
    type Error = crate::Error;

    fn try_from(log: u32) -> core::result::Result<Self, Self::Error> {
        Self::new(log).ok_or_else(|| {
            crate::Error::invalid_configuration(format!(
                "zstd window log {log} is out of range; expected {}..={}",
                Self::MIN.get(),
                Self::MAX.get()
            ))
        })
    }
}

/// Settings that only zstd has.
///
/// # Examples
///
/// ```
/// use compressors::Resources;
/// use compressors::zstd::{self, CompressionLevel};
///
/// // A negative fast level -- below the portable scale, so only reachable here.
/// let compressor = zstd::Compressor::builder()
///     .compression_level(CompressionLevel::new(-3).expect("-3 is in range"))
///     .build(&Resources::default())?;
/// # let _ = compressor;
/// # Ok::<(), compressors::BuildError>(())
/// ```
impl crate::CompressorBuilder<Zstd> {
    /// Sets the level on zstd's own scale, overriding any portable [`Level`][crate::Level].
    ///
    /// Use this only when you need a level the portable scale does not reach; prefer
    /// [`level`][crate::CompressorBuilder::level] otherwise, so the same configuration keeps working if the
    /// format changes.
    #[must_use]
    pub const fn compression_level(mut self, level: CompressionLevel) -> Self {
        self.format.level = Some(level);
        self
    }
}

impl crate::DecompressorBuilder<Zstd> {
    /// Limits the largest frame window this decompressor accepts.
    #[must_use]
    pub const fn max_window_log(mut self, max_window_log: WindowLog) -> Self {
        self.format.max_window_log = Some(max_window_log);
        self
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod configuration_tests {
    use super::*;

    #[test]
    fn accepts_the_full_native_level_range() {
        assert!(CompressionLevel::min().get() < 0);
        assert_eq!(CompressionLevel::new(CompressionLevel::min().get()), Some(CompressionLevel::min()));
        assert_eq!(CompressionLevel::new(CompressionLevel::max().get()), Some(CompressionLevel::max()));
        assert_eq!(CompressionLevel::default(), CompressionLevel::DEFAULT);
        assert_eq!(
            CompressionLevel::try_from(CompressionLevel::DEFAULT.get()).expect("in range"),
            CompressionLevel::DEFAULT
        );
        assert_eq!(i32::from(CompressionLevel::DEFAULT), CompressionLevel::DEFAULT.get());

        let error = CompressionLevel::try_from(CompressionLevel::max().get().saturating_add(1)).expect_err("out of range");
        assert!(error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn validates_window_logs() {
        assert_eq!(WindowLog::new(WindowLog::MIN.get()), Some(WindowLog::MIN));
        assert_eq!(WindowLog::new(WindowLog::MAX.get()), Some(WindowLog::MAX));
        assert_eq!(WindowLog::new(WindowLog::MIN.get() - 1), None);
        assert_eq!(WindowLog::new(WindowLog::MAX.get() + 1), None);
        assert_eq!(WindowLog::default(), WindowLog::DEFAULT);
        assert_eq!(WindowLog::try_from(WindowLog::DEFAULT.get()).expect("in range"), WindowLog::DEFAULT);

        let error = WindowLog::try_from(WindowLog::MIN.get() - 1).expect_err("out of range");
        assert!(error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn max_window_log_matches_this_targets_pointer_width() {
        // Computed independently of `WindowLog::MAX`'s own definition so a mutated comparison
        // there cannot hide behind a test that recomputes the same expression.
        let expected = if usize::BITS == 32 { 30 } else { 31 };
        assert_eq!(WindowLog::MAX.get(), expected);
    }
}
