// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared compressor and decompressor builders.
//!
//! [`CompressorBuilder`] and [`DecompressorBuilder`] store settings that apply to every format.
//! These settings include the level, output chunk size, decompression limits, and trailing-data
//! policy. The type parameter records the selected format.
//!
//! [`Resources`][crate::Resources] supplies memory and an engine pool to `build`.
//!
//! `CompressorBuilder<()>` has no selected format. Each enabled format adds a `build_gzip`-style
//! method. `build_format` accepts a `Format` value at run time.
//!
//! `CompressorBuilder<Gzip>` selects gzip. A format-specific builder adds its format settings and a
//! `build` method that returns the concrete compressor.

use std::num::NonZeroUsize;

use crate::level::Level;
use crate::limits::DecompressorLimits;
use crate::trailing::TrailingData;

/// How much output a single `pull` produces before handing control back.
///
/// This bounds pending output only. The adapter retains at most one output chunk. The caller can
/// retain more chunks after it receives them. Pending input and the engine's own window and tables
/// are additional, and their size depends on the format and its configuration.
///
/// The 64 KiB default balances per-call overhead, output latency, and retained memory. The
/// `chunk_size` benchmark group measures overhead and buffer churn. A caller with a different
/// latency or memory budget can use [`output_chunk_size`][CompressorBuilder::output_chunk_size].
pub(crate) const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Configures a compressor.
///
/// The type parameter selects the format. Its default is `()`, which means that no format is
/// selected. This shared builder provides one `build_gzip`-style method for each enabled format and
/// [`build_format`][CompressorBuilder::build_format]. A format-specific builder also provides its
/// format settings and a `build` method for its concrete compressor.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use compressors::format::Format;
/// use compressors::{CompressorBuilder, Level, Resources};
///
/// // Apply shared settings to a format selected at run time.
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
    /// The selected format settings, or `()` before format selection.
    ///
    /// The shared builder passes this value to the engine. The format module defines its setters.
    #[cfg_attr(
        not(any(test, any_format)),
        expect(dead_code, reason = "only a format module's build method reads the settings it owns")
    )]
    pub(crate) format: T,
}

impl<T> CompressorBuilder<T> {
    /// Starts from the shared defaults, with one format's settings already chosen.
    ///
    /// Each format module calls this from its [`Default`] implementation. Marker types therefore
    /// do not need public constructors.
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
    /// This bounds output that waits for `pull` to return. It does not bound pending input or engine
    /// state. Larger chunks reduce per-call overhead. Smaller chunks reduce output latency and
    /// retained output memory.
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
        not(any(test, any_format)),
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
/// The type parameter selects the format. Its default is `()`, which means that no format is
/// selected. This shared builder provides one `build_gzip`-style method for each enabled format and
/// [`build_format`][DecompressorBuilder::build_format]. A format-specific builder also provides its
/// format settings and a `build` method for its concrete decompressor.
///
/// # Security
///
/// Compressed data can expand by orders of magnitude. Untrusted input can therefore exhaust memory.
/// The risk depends on retained output, not throughput. `CompressionStream` returns one bounded
/// chunk at a time. A consumer stays bounded if it processes and drops each chunk.
///
/// Set [`limits`][DecompressorBuilder::limits] with
/// [`max_output_len`][DecompressorLimits::max_output_len] to what you can afford whenever
/// decompressed output is accumulated. This includes whole-buffer functions and consumers that
/// retain stream chunks. A cumulative cap on a pipeline that retains nothing can reject valid long
/// streams.
#[derive(Debug, Clone)]
pub struct DecompressorBuilder<T = ()> {
    pub(crate) limits: DecompressorLimits,
    pub(crate) chunk_size: NonZeroUsize,
    pub(crate) multi_stream: Option<bool>,
    pub(crate) trailing_data: TrailingData,
    /// The selected format settings, or `()` before format selection.
    #[cfg_attr(
        not(any(test, any_format)),
        expect(dead_code, reason = "only a format module's build method reads the settings it owns")
    )]
    pub(crate) format: T,
}

impl<T> DecompressorBuilder<T> {
    /// Starts from the shared defaults, with one format's settings already chosen.
    ///
    /// Each format module calls this from its [`Default`] implementation. Marker types therefore
    /// do not need public constructors.
    pub(crate) fn with_format(format: T) -> Self {
        Self {
            limits: DecompressorLimits::new(),
            chunk_size: default_chunk_size(),
            multi_stream: None,
            trailing_data: TrailingData::Reject,
            format,
        }
    }

    /// Overrides the bounds on how much data decompression can produce.
    ///
    /// Unset bounds keep the selected format defaults. Most formats define only a ratio default.
    /// Brotli has no structural expansion ceiling, so it defines no ratio default. Whole-buffer
    /// functions add output and stream fallbacks. See [`DecompressorLimits`].
    ///
    /// # Security
    ///
    /// Set [`max_output_len`][DecompressorLimits::max_output_len] to match your memory
    /// budget whenever you accumulate decompressed output. A cumulative cap protects retained
    /// output. No format defines a default for this bound. Whole-buffer functions add a 64 MiB
    /// fallback when the bound is unset.
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
    /// If left unset, each format keeps its default. Gzip and zstd enable concatenation to match
    /// `gzip(1)` and the `zstd` command-line tool. The other formats disable concatenation.
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
    /// The default is [`TrailingData::Reject`]. A stream that does not end with the input returns an
    /// error. Use [`TrailingData::Ignore`] for a container that places other data after the
    /// compressed stream.
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
        not(any(test, any_format)),
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
    // The compiler evaluates this expression. A zero `DEFAULT_CHUNK_SIZE` fails the build, so no
    // runtime branch exists.
    const CHUNK_SIZE: NonZeroUsize = match NonZeroUsize::new(DEFAULT_CHUNK_SIZE) {
        Some(size) => size,
        None => panic!("DEFAULT_CHUNK_SIZE must not be zero"),
    };

    CHUNK_SIZE
}
