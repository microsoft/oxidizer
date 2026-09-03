// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The builders every format shares.
//!
//! [`CompressorBuilder`] and [`DecompressorBuilder`] hold the settings that mean the same thing
//! whichever format ends up being used: the level, the output chunk size, the decompression limits
//! and the trailing-data policy. The type parameter records whether a format has been chosen yet.
//!
//! What a codec is built *with* -- memory and an engine pool -- is not a setting, so it lives in
//! [`Resources`][crate::Resources] and is supplied to `build` instead.
//!
//! `CompressorBuilder<()>` has not chosen one. It is the builder to hold when the format is a
//! runtime decision: each enabled format's module adds a `build_gzip`-style method to it, beside
//! `build_format` for a `Format` value.
//!
//! `CompressorBuilder<Gzip>` has. Committing to a format -- which `gzip::Compressor::builder()`
//! does -- adds the settings only that format has, along with a `build` method returning that
//! format's own compressor rather than a boxed one.

use std::num::NonZeroUsize;

use crate::level::Level;
use crate::limits::DecompressorLimits;
use crate::trailing::TrailingData;

/// How much output a single `pull` produces before handing control back.
///
/// This bounds a codec's working set: a caller streaming hundreds of gigabytes never holds more
/// than one pending input view plus one chunk of output.
pub(crate) const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Configures a compressor.
///
/// The type parameter selects the format, and defaults to `()` for a builder that has not chosen
/// one yet: it carries only the settings every format shares, and gains a `build_gzip`-style method
/// per enabled format plus [`build_format`][CompressorBuilder::build_format]. Committing to a
/// format -- which [`gzip::Compressor::builder`][crate::gzip::Compressor::builder] does -- adds that
/// format's own settings and a `build` returning its concrete compressor.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use compressors::format::Format;
/// use compressors::{CompressorBuilder, Level, Resources};
///
/// // Settings that say nothing about the format, applied to one chosen at runtime.
/// let settings = CompressorBuilder::new().level(Level::HIGH);
/// let compressor = settings.build_format(Format::Gzip, Resources::global())?;
/// # let _ = compressor;
/// # }
/// # Ok::<(), compressors::BuildError>(())
/// ```
#[derive(Debug, Clone)]
pub struct CompressorBuilder<T = ()> {
    pub(crate) level: Level,
    pub(crate) chunk_size: NonZeroUsize,
    /// The chosen format's own settings, and `()` until a format is chosen.
    ///
    /// The shared builder never reads this beyond handing it to the codec; the format's own module
    /// adds the setters that populate it.
    #[cfg_attr(
        not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")),
        expect(dead_code, reason = "only a format module's build method reads the settings it owns")
    )]
    pub(crate) format: T,
}

impl<T> CompressorBuilder<T> {
    /// Starts from the shared defaults, with one format's settings already chosen.
    ///
    /// Each format's module wraps this in its own [`Default`] implementation, which is why the
    /// marker types need no public constructor.
    pub(crate) fn with_format(format: T) -> Self {
        Self {
            level: Level::DEFAULT,
            chunk_size: default_chunk_size(),
            format,
        }
    }

    /// Sets the compression level, mapped onto the format's native range.
    #[must_use]
    pub const fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Sets how much output a single `pull` produces before returning.
    ///
    /// This bounds the compressor's working set. Larger chunks reduce per-call overhead; smaller
    /// chunks reduce peak memory and latency.
    #[must_use]
    pub const fn output_chunk_size(mut self, bytes: NonZeroUsize) -> Self {
        self.chunk_size = bytes;
        self
    }
}

impl CompressorBuilder<()> {
    /// Starts configuring a compressor whose format has not been chosen yet.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Commits the format-independent settings to one format.
    #[cfg_attr(
        not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")),
        expect(dead_code, reason = "only a format module's build method commits a builder to a format")
    )]
    pub(crate) fn specialize<T>(self, format: T) -> CompressorBuilder<T> {
        CompressorBuilder {
            level: self.level,
            chunk_size: self.chunk_size,
            format,
        }
    }
}

impl Default for CompressorBuilder<()> {
    #[inline]
    fn default() -> Self {
        Self::with_format(())
    }
}

/// Configures a decompressor.
///
/// The type parameter selects the format, and defaults to `()` for a builder that has not chosen
/// one yet: it carries only the settings every format shares, and gains a `build_gzip`-style method
/// per enabled format plus [`build_format`][DecompressorBuilder::build_format]. Committing to a
/// format -- which [`gzip::Decompressor::builder`][crate::gzip::Decompressor::builder] does -- adds
/// that format's own settings and a `build` returning its concrete decompressor.
///
/// # Security
///
/// Compressed data can expand by orders of magnitude, so a decompressor pointed at untrusted input
/// is a memory-exhaustion vector. Set [`limits`][DecompressorBuilder::limits] with
/// [`with_max_output_len`][DecompressorLimits::with_max_output_len] when the data comes from an
/// untrusted peer.
#[derive(Debug, Clone)]
pub struct DecompressorBuilder<T = ()> {
    pub(crate) limits: DecompressorLimits,
    pub(crate) chunk_size: NonZeroUsize,
    pub(crate) multi_stream: Option<bool>,
    pub(crate) trailing_data: TrailingData,
    /// The chosen format's own settings, and `()` until a format is chosen.
    #[cfg_attr(
        not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")),
        expect(dead_code, reason = "only a format module's build method reads the settings it owns")
    )]
    pub(crate) format: T,
}

impl<T> DecompressorBuilder<T> {
    /// Starts from the shared defaults, with one format's settings already chosen.
    ///
    /// Each format's module wraps this in its own [`Default`] implementation, which is why the
    /// marker types need no public constructor.
    pub(crate) fn with_format(format: T) -> Self {
        Self {
            limits: DecompressorLimits::new(),
            chunk_size: default_chunk_size(),
            multi_stream: None,
            trailing_data: TrailingData::Reject,
            format,
        }
    }

    /// Overrides the bounds on how much data decompression may produce.
    ///
    /// Bounds left unset on the passed value keep the chosen format's own defaults. For most
    /// formats that is a ratio and nothing else; brotli has no structural ceiling to derive one
    /// from, so it defaults to no ratio bound either. The conveniences that buffer a whole result
    /// add their own output and stream caps on top; see [`DecompressorLimits`].
    ///
    /// # Security
    ///
    /// Set [`with_max_output_len`][DecompressorLimits::with_max_output_len] when the data comes
    /// from an untrusted peer. Do not rely on the format default for brotli, which has none.
    #[must_use]
    pub const fn limits(mut self, limits: DecompressorLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets how much output a single `pull` produces before returning.
    #[must_use]
    pub const fn output_chunk_size(mut self, bytes: NonZeroUsize) -> Self {
        self.chunk_size = bytes;
        self
    }

    /// Sets whether consecutive streams decompress as one logical stream.
    ///
    /// Left unset, each format keeps its own default: enabled for gzip and zstd, matching `gzip(1)`
    /// and the `zstd` command line tool, and disabled for the rest, where concatenation is not an
    /// established convention.
    ///
    /// When enabled, bytes after a complete stream must begin another valid stream, so trailing
    /// padding is reported as corrupt data. When disabled,
    /// [`trailing_data`][DecompressorBuilder::trailing_data] decides what happens to those bytes.
    #[must_use]
    pub const fn multi_stream(mut self, enabled: bool) -> Self {
        self.multi_stream = Some(enabled);
        self
    }

    /// Sets how a single-stream decompressor handles bytes after the compressed stream.
    ///
    /// Defaults to [`TrailingData::Reject`], so a stream that does not end exactly at end of input
    /// is an error rather than a silent truncation of what the caller was given. Pass
    /// [`TrailingData::Ignore`] for a container whose framing legitimately puts other data after
    /// the compressed stream.
    ///
    /// In multi-stream mode, subsequent bytes are interpreted as another compressed stream
    /// regardless of this setting.
    #[must_use]
    pub const fn trailing_data(mut self, trailing_data: TrailingData) -> Self {
        self.trailing_data = trailing_data;
        self
    }
}

impl DecompressorBuilder<()> {
    /// Starts configuring a decompressor whose format has not been chosen yet.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Commits the format-independent settings to one format.
    #[cfg_attr(
        not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")),
        expect(dead_code, reason = "only a format module's build method commits a builder to a format")
    )]
    pub(crate) fn specialize<T>(self, format: T) -> DecompressorBuilder<T> {
        DecompressorBuilder {
            limits: self.limits,
            chunk_size: self.chunk_size,
            multi_stream: self.multi_stream,
            trailing_data: self.trailing_data,
            format,
        }
    }
}

impl Default for DecompressorBuilder<()> {
    #[inline]
    fn default() -> Self {
        Self::with_format(())
    }
}

const fn default_chunk_size() -> NonZeroUsize {
    // Evaluated by the compiler: if `DEFAULT_CHUNK_SIZE` were ever zero, this constant would fail
    // to build rather than panicking at runtime, so there is no runtime branch to cover here.
    const CHUNK_SIZE: NonZeroUsize = match NonZeroUsize::new(DEFAULT_CHUNK_SIZE) {
        Some(size) => size,
        None => panic!("DEFAULT_CHUNK_SIZE must not be zero"),
    };

    CHUNK_SIZE
}
