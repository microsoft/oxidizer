// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Zstandard (RFC 8878): fast compression with ratios well beyond the deflate family.
//!
//! The usual choice when both speed and ratio matter, and the format behind HTTP
//! `Content-Encoding: zstd`. Requires the `zstd` cargo feature.
//!
//! Unlike this crate's other formats, zstd is provided by a C library compiled from bundled
//! sources, so enabling it requires a C compiler. Builds that leave the feature off stay pure Rust.
//!
//! # Examples
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::zstd;
//!
//! let memory = GlobalPool::new();
//! let compressed = zstd::compress(
//!     BytesView::copied_from_slice(b"the quick brown fox", &memory),
//!     memory.clone(),
//! )?;
//! assert_eq!(compressed.range(0..4).to_vec(), vec![0x28, 0xb5, 0x2f, 0xfd]);
//!
//! assert_eq!(
//!     zstd::decompress(compressed, memory)?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

mod codec;

use crate::format::macros::define_format;
use crate::limits::FormatLimits;
use crate::zstd::codec::{ZstdCompress, ZstdDecompress};

/// Zstd's default bounds.
///
/// Zstd has no structural expansion ceiling, so like brotli it needs a far looser ratio than the
/// deflate family. This is a coarse backstop rather than real protection; see
/// [`DecompressionLimits`] for what actually bounds an untrusted stream.
const DEFAULT_LIMITS: FormatLimits = FormatLimits::new(Some(250_000), None);

define_format! {
    name = "zstd",
    compressor_codec = ZstdCompress,
    compressor_options = CompressorOptions,
    new_compressor = ZstdCompress::new,
    decompressor_codec = ZstdDecompress,
    decompressor_options = DecompressorOptions,
    default_limits = DEFAULT_LIMITS,
    new_decompressor = ZstdDecompress::new,
    multi_stream_default = true,
    multi_stream_doc = "Sets whether concatenated zstd frames decompress as one logical stream.\n\nEnabled by default, matching the `zstd` command line tool.",
}

/// A level on zstd's own scale, for reaching settings the portable [`Level`] does not cover.
///
/// The portable scale is anchored on zstd's default so that [`Level::DEFAULT`] means the same
/// thing on every format. Native negative fast modes and levels above the portable range remain
/// reachable here. Strong levels are rarely worth it -- measured on realistic JSON, level 19 is
/// over 200 times slower than level 3 for about `17%` better compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompressionLevel(i32);

impl CompressionLevel {
    /// Zstd's own default, which the portable [`Level::DEFAULT`] also maps to.
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
    pub fn min() -> Self {
        Self(zstd_safe::min_c_level())
    }

    /// The strongest level supported by the bundled zstd library.
    #[must_use]
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

/// Zstd's format-specific compressor settings.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CompressorOptions {
    pub(crate) level: Option<CompressionLevel>,
}

/// Zstd's format-specific decompressor settings.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DecompressorOptions {
    pub(crate) max_window_log: Option<WindowLog>,
}

/// Settings that only zstd has.
///
/// # Examples
///
/// ```
/// use bytesbuf::mem::GlobalPool;
/// use compressors::zstd::{self, CompressionLevel};
///
/// let compressor = zstd::Compressor::builder()
///     .compression_level(CompressionLevel::new(19).expect("19 is in range"))
///     .build(GlobalPool::new());
/// # let _ = compressor;
/// ```
impl CompressorBuilder {
    /// Sets the level on zstd's own scale, overriding any portable [`Level`].
    ///
    /// Use this only when you need a level the portable scale does not reach; prefer
    /// [`level`][CompressorBuilder::level] otherwise, so the same configuration keeps working if the
    /// format changes.
    #[must_use]
    pub const fn compression_level(mut self, level: CompressionLevel) -> Self {
        self.options.level = Some(level);
        self
    }
}

impl DecompressorBuilder {
    /// Limits the largest frame window this decompressor accepts.
    #[must_use]
    pub const fn max_window_log(mut self, max_window_log: WindowLog) -> Self {
        self.options.max_window_log = Some(max_window_log);
        self
    }
}

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
}
