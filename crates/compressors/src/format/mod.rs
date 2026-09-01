// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Choosing a compression format at runtime.
//!
//! The format modules (`gzip` and friends) are the right choice when the format is
//! known at compile time. This module is for when it is not: encoding whatever a client asked for,
//! or decoding whatever a peer declared it sent.
//!
//! [`Format`] is the entry point. The builders it returns live here beside it, so they do not
//! collide with the per-format builders such as
//! [`gzip::CompressorBuilder`][crate::gzip::CompressorBuilder].

#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
pub(crate) mod macros;

use std::num::NonZeroUsize;

use bytesbuf::BytesView;
use bytesbuf::mem::MemoryShared;

use crate::compression::{Compressing, Compression, Decompressing};
use crate::engine::DEFAULT_CHUNK_SIZE;
use crate::error::Result;
use crate::level::Level;
use crate::limits::DecompressionLimits;
use crate::pool::Pool;
use crate::trailing::TrailingData;

/// A compression format, selectable at runtime.
///
/// The format modules (`gzip` and friends) are the right choice when the format is
/// known at compile time. This enum is for when it is not: encoding whatever a client asked for,
/// or decoding whatever a peer declared it sent.
///
/// # Examples
///
/// ```
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::GlobalPool;
/// use compressors::Level;
/// use compressors::format::Format;
///
/// // The format arrives as a string, from an HTTP header.
/// let format = Format::from_content_encoding("gzip").expect("a supported encoding");
///
/// let memory = GlobalPool::new();
/// let mut compressor = format.compressor().level(Level::HIGH).build(memory.clone());
///
/// compressor.push(BytesView::copied_from_slice(b"payload", &memory))?;
/// # Ok::<(), compressors::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// Raw deflate, RFC 1951. See `deflate`. Requires the `deflate` feature.
    #[cfg(feature = "deflate")]
    Deflate,
    /// Zlib, RFC 1950. See `zlib`. Requires the `zlib` feature.
    #[cfg(feature = "zlib")]
    Zlib,
    /// Gzip, RFC 1952. See `gzip`. Requires the `gzip` feature.
    #[cfg(feature = "gzip")]
    Gzip,
    /// Brotli, RFC 7932. See `brotli`. Requires the `brotli` feature.
    #[cfg(feature = "brotli")]
    Brotli,
    /// Zstandard, RFC 8878. See `zstd`. Requires the `zstd` feature.
    #[cfg(feature = "zstd")]
    Zstd,
}

impl Format {
    /// Every format this build supports, in no particular order.
    ///
    /// The contents depend on which cargo features are enabled.
    pub const ALL: &'static [Self] = &[
        #[cfg(feature = "deflate")]
        Self::Deflate,
        #[cfg(feature = "zlib")]
        Self::Zlib,
        #[cfg(feature = "gzip")]
        Self::Gzip,
        #[cfg(feature = "brotli")]
        Self::Brotli,
        #[cfg(feature = "zstd")]
        Self::Zstd,
    ];

    /// The HTTP `Content-Encoding` token for this format, if it has one.
    ///
    /// Returns `None` for `Format::Deflate`: raw deflate has no HTTP token. Note that the HTTP
    /// `deflate` token means a *zlib* stream, not raw deflate, so it maps to `Format::Zlib`.
    #[must_use]
    #[cfg_attr(
        not(feature = "deflate"),
        expect(
            clippy::unnecessary_wraps,
            reason = "raw deflate is the only format without an HTTP token, and it is not enabled in this configuration"
        )
    )]
    pub const fn content_encoding(self) -> Option<&'static str> {
        match self {
            #[cfg(feature = "deflate")]
            Self::Deflate => None,
            #[cfg(feature = "zlib")]
            Self::Zlib => Some("deflate"),
            #[cfg(feature = "gzip")]
            Self::Gzip => Some("gzip"),
            #[cfg(feature = "brotli")]
            Self::Brotli => Some("br"),
            #[cfg(feature = "zstd")]
            Self::Zstd => Some("zstd"),
        }
    }

    /// Parses a single HTTP `Content-Encoding` token.
    ///
    /// Matching is case-insensitive, as HTTP requires. `deflate` maps to `Format::Zlib`, which is
    /// what the token actually denotes; `x-gzip` is accepted as a legacy alias for `gzip`. Tokens
    /// for formats this build does not support return `None`.
    ///
    /// This takes one bare token rather than parsing a complete HTTP header.
    #[must_use]
    pub fn from_content_encoding(token: &str) -> Option<Self> {
        let token = token.trim();

        #[cfg(feature = "gzip")]
        if token.eq_ignore_ascii_case("gzip") || token.eq_ignore_ascii_case("x-gzip") {
            return Some(Self::Gzip);
        }

        #[cfg(feature = "zlib")]
        if token.eq_ignore_ascii_case("deflate") {
            return Some(Self::Zlib);
        }

        #[cfg(feature = "brotli")]
        if token.eq_ignore_ascii_case("br") {
            return Some(Self::Brotli);
        }

        #[cfg(feature = "zstd")]
        if token.eq_ignore_ascii_case("zstd") {
            return Some(Self::Zstd);
        }

        #[cfg(not(any(feature = "brotli", feature = "gzip", feature = "zlib", feature = "zstd")))]
        let _ = token;

        None
    }

    /// Starts configuring a compressor for this format.
    #[must_use]
    pub const fn compressor(self) -> CompressorBuilder {
        CompressorBuilder {
            format: self,
            level: Level::DEFAULT,
            chunk_size: default_chunk_size(),
            pool: None,
        }
    }

    /// Starts configuring a decompressor for this format.
    #[must_use]
    pub const fn decompressor(self) -> DecompressorBuilder {
        DecompressorBuilder {
            format: self,
            limits: DecompressionLimits::new(),
            chunk_size: default_chunk_size(),
            multi_stream: None,
            trailing_data: TrailingData::Preserve,
            pool: None,
        }
    }

    /// Compresses a complete byte sequence that is already in memory.
    ///
    /// Uses [`Level::DEFAULT`]; for anything else, configure a compressor with [`Format::compressor`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying compression engine fails.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "one-shot operations consistently borrow the selected runtime format"
    )]
    pub fn compress(&self, input: BytesView, memory: impl MemoryShared) -> Result<BytesView> {
        (*self).compressor().build(memory).compress(input)
    }

    /// Decompresses a complete stream that is already in memory.
    ///
    /// Applies [`DecompressionLimits::new()`]; for anything else, configure a decompressor with
    /// [`Format::decompressor`].
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed, truncated, or exceeds the default limits.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "one-shot operations consistently borrow the selected runtime format"
    )]
    pub fn decompress(&self, input: BytesView, memory: impl MemoryShared) -> Result<BytesView> {
        (*self).decompressor().build(memory).decompress(input)
    }

    /// Decompresses a complete stream with explicit output limits.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed, truncated, or exceeds `limits`.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "one-shot operations consistently borrow the selected runtime format"
    )]
    pub fn decompress_with_limits(&self, input: BytesView, memory: impl MemoryShared, limits: DecompressionLimits) -> Result<BytesView> {
        (*self).decompressor().limits(limits).build(memory).decompress(input)
    }
}

const fn default_chunk_size() -> NonZeroUsize {
    match NonZeroUsize::new(DEFAULT_CHUNK_SIZE) {
        Some(size) => size,
        None => NonZeroUsize::MIN,
    }
}

/// Configures a compressor for a [`Format`] chosen at runtime.
///
/// Mirrors the per-format builders such as [`gzip::CompressorBuilder`][crate::gzip::CompressorBuilder],
/// but produces a boxed [`Compressing`] operation so the format need not be known at compile time. Reach it
/// through [`Format::compressor`] rather than naming it directly.
#[derive(Debug, Clone)]
pub struct CompressorBuilder {
    format: Format,
    level: Level,
    chunk_size: NonZeroUsize,
    pool: Option<Pool>,
}

impl CompressorBuilder {
    /// Sets the compression level, mapped onto the format's native range.
    #[must_use]
    pub const fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Sets how much output a single `pull` produces before returning.
    #[must_use]
    pub const fn output_chunk_size(mut self, bytes: NonZeroUsize) -> Self {
        self.chunk_size = bytes;
        self
    }

    /// Recycles engine state through a shared [`Pool`].
    ///
    /// Building a compressor is not free, so a service that compresses many messages should hand every
    /// compressor the same pool. The engine is returned when the compressor is dropped. Without a pool
    /// each compressor builds its own engine, which is the default.
    #[must_use]
    pub fn pool(mut self, pool: Pool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Builds the compressor, drawing its output buffers from `memory`.
    #[must_use]
    pub fn build(self, memory: impl MemoryShared) -> Box<dyn Compressing> {
        macro_rules! build {
            ($module:ident) => {{
                let builder = crate::$module::Compressor::builder()
                    .level(self.level)
                    .output_chunk_size(self.chunk_size);

                let builder = match self.pool {
                    Some(pool) => builder.pool(pool),
                    None => builder,
                };

                Box::new(builder.build(memory))
            }};
        }

        match self.format {
            #[cfg(feature = "deflate")]
            Format::Deflate => build!(deflate),
            #[cfg(feature = "zlib")]
            Format::Zlib => build!(zlib),
            #[cfg(feature = "gzip")]
            Format::Gzip => build!(gzip),
            #[cfg(feature = "brotli")]
            Format::Brotli => build!(brotli),
            #[cfg(feature = "zstd")]
            Format::Zstd => build!(zstd),
        }
    }
}

/// Configures a decompressor for a [`Format`] chosen at runtime.
///
/// Mirrors the per-format builders such as [`gzip::DecompressorBuilder`][crate::gzip::DecompressorBuilder],
/// but produces a boxed [`Decompressing`] operation so the format need not be known at compile time. Reach it
/// through [`Format::decompressor`] rather than naming it directly.
#[derive(Debug, Clone)]
pub struct DecompressorBuilder {
    format: Format,
    limits: DecompressionLimits,
    chunk_size: NonZeroUsize,
    multi_stream: Option<bool>,
    trailing_data: TrailingData,
    pool: Option<Pool>,
}

impl DecompressorBuilder {
    /// Overrides the bounds on how much data decompression may produce.
    ///
    /// Bounds left unset on the passed value keep the chosen format's own defaults, which differ by
    /// orders of magnitude between the deflate family and brotli.
    ///
    /// # Security
    ///
    /// Set [`with_max_output_len`][DecompressionLimits::with_max_output_len] when the data comes
    /// from an untrusted peer.
    #[must_use]
    pub const fn limits(mut self, limits: DecompressionLimits) -> Self {
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
    /// Left unset, each format keeps its own default: enabled for `Format::Gzip` and
    /// `Format::Zstd`, matching `gzip(1)` and the `zstd` tool, and disabled for the rest, where
    /// concatenation is not an established convention.
    ///
    /// When enabled, bytes after a complete stream must begin another valid stream. When disabled,
    /// [`DecompressorBuilder::trailing_data`] controls how trailing bytes are handled.
    #[must_use]
    pub const fn multi_stream(mut self, enabled: bool) -> Self {
        self.multi_stream = Some(enabled);
        self
    }

    /// Sets how a single-stream decompressor handles trailing bytes.
    ///
    /// In multi-stream mode, subsequent bytes are always interpreted as another compressed stream.
    #[must_use]
    pub const fn trailing_data(mut self, trailing_data: TrailingData) -> Self {
        self.trailing_data = trailing_data;
        self
    }

    /// Recycles engine state through a shared [`Pool`].
    ///
    /// The engine is returned when the decompressor is dropped. See [`Pool`] for which engines are
    /// actually recycled.
    #[must_use]
    pub fn pool(mut self, pool: Pool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Builds the decompressor, drawing its output buffers from `memory`.
    #[must_use]
    pub fn build(self, memory: impl MemoryShared) -> Box<dyn Decompressing> {
        macro_rules! build {
            ($module:ident) => {{
                let builder = crate::$module::Decompressor::builder()
                    .limits(self.limits)
                    .output_chunk_size(self.chunk_size);

                let builder = match self.multi_stream {
                    Some(enabled) => builder.multi_stream(enabled),
                    None => builder,
                };
                let builder = builder.trailing_data(self.trailing_data);

                let builder = match self.pool {
                    Some(pool) => builder.pool(pool),
                    None => builder,
                };

                Box::new(builder.build(memory))
            }};
        }

        match self.format {
            #[cfg(feature = "deflate")]
            Format::Deflate => build!(deflate),
            #[cfg(feature = "zlib")]
            Format::Zlib => build!(zlib),
            #[cfg(feature = "gzip")]
            Format::Gzip => build!(gzip),
            #[cfg(feature = "brotli")]
            Format::Brotli => build!(brotli),
            #[cfg(feature = "zstd")]
            Format::Zstd => build!(zstd),
        }
    }
}

#[cfg(test)]
mod tests {
    use bytesbuf::mem::GlobalPool;

    use super::*;
    use crate::Output;

    fn view(bytes: &[u8]) -> BytesView {
        BytesView::copied_from_slice(bytes, &GlobalPool::new())
    }

    fn compressed_len(builder: CompressorBuilder, payload: &[u8]) -> usize {
        let mut compressor = builder.build(GlobalPool::new());
        compressor.push(view(payload)).expect("push succeeds");
        compressor.end_input();

        let mut total = 0;
        loop {
            match compressor.pull().expect("pull succeeds") {
                Output::Data(chunk) => total += chunk.len(),
                Output::Progress => {}
                Output::NeedInput => panic!("compressor requested input after end"),
                Output::Done => break,
            }
        }

        total
    }

    #[test]
    fn every_format_round_trips_through_the_enum() {
        let payload = b"runtime selected format ".repeat(200);

        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let compressed = format.compress(view(&payload), memory.clone()).expect("compression succeeds");
            let plain = format.decompress(compressed, memory).expect("decompression succeeds");

            assert_eq!(plain.to_vec(), payload, "{format:?} failed to round trip");
        }
    }

    #[test]
    fn content_encoding_tokens_round_trip() {
        for &format in Format::ALL {
            let Some(token) = format.content_encoding() else {
                continue;
            };

            assert_eq!(
                Format::from_content_encoding(token),
                Some(format),
                "{format:?} did not survive its own token"
            );
        }
    }

    #[cfg(all(feature = "deflate", feature = "zlib"))]
    #[test]
    fn http_deflate_token_means_zlib() {
        // The most common source of confusion in this area: the HTTP `deflate` token denotes a zlib
        // stream, not raw deflate.
        assert_eq!(Format::from_content_encoding("deflate"), Some(Format::Zlib));
        assert_eq!(Format::Deflate.content_encoding(), None);
    }

    #[cfg(feature = "gzip")]
    #[test]
    fn content_encoding_parsing_is_case_insensitive_and_trims() {
        assert_eq!(Format::from_content_encoding("GZIP"), Some(Format::Gzip));
        assert_eq!(Format::from_content_encoding("  gzip  "), Some(Format::Gzip));
        assert_eq!(Format::from_content_encoding("x-gzip"), Some(Format::Gzip));
        assert_eq!(Format::from_content_encoding("identity"), None);
        assert_eq!(Format::from_content_encoding(""), None);
    }

    #[cfg(feature = "brotli")]
    #[test]
    fn brotli_uses_the_br_token() {
        assert_eq!(Format::from_content_encoding("br"), Some(Format::Brotli));
        assert_eq!(Format::Brotli.content_encoding(), Some("br"));
    }

    #[cfg(not(feature = "brotli"))]
    #[test]
    fn brotli_token_is_rejected_when_the_feature_is_off() {
        assert_eq!(Format::from_content_encoding("br"), None);
    }

    #[cfg(not(feature = "gzip"))]
    #[test]
    fn gzip_token_is_rejected_when_the_feature_is_off() {
        assert_eq!(Format::from_content_encoding("gzip"), None);
    }

    #[test]
    fn the_compressor_builder_applies_its_level() {
        let payload = b"the quick brown fox jumps over the lazy dog ".repeat(400);

        for &format in Format::ALL {
            let fast = compressed_len(format.compressor().level(Level::FAST), &payload);
            let best = compressed_len(format.compressor().level(Level::HIGH), &payload);

            assert!(best <= fast, "{format:?}: best={best} should not exceed fast={fast}");
        }
    }

    #[test]
    fn the_compressor_builder_applies_its_chunk_size() {
        let bound = NonZeroUsize::new(128).expect("128 is not zero");

        for &format in Format::ALL {
            let mut compressor = format.compressor().output_chunk_size(bound).build(GlobalPool::new());
            compressor.push(view(&b"chunked ".repeat(5_000))).expect("push succeeds");
            compressor.end_input();

            loop {
                match compressor.pull().expect("pull succeeds") {
                    Output::Data(chunk) => {
                        assert!(chunk.len() <= bound.get(), "{format:?} produced a {} byte chunk", chunk.len());
                    }
                    Output::Progress => {}
                    Output::NeedInput => panic!("compressor requested input after end"),
                    Output::Done => break,
                }
            }
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_limits() {
        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let compressed = format
                .compress(view(&vec![0_u8; 4 * 1024 * 1024]), memory.clone())
                .expect("compression succeeds");

            let mut decompressor = format
                .decompressor()
                .limits(DecompressionLimits::new().without_max_ratio().with_max_output_len(1024))
                .build(memory);
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            let error = loop {
                match decompressor.pull() {
                    Ok(Output::Data(_) | Output::Progress) => {}
                    Ok(_) => panic!("{format:?}: the cap should have fired"),
                    Err(error) => break error,
                }
            };

            assert!(error.is_limit_exceeded(), "{format:?}: got {error}");
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_chunk_size() {
        let bound = NonZeroUsize::new(128).expect("128 is not zero");

        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let compressed = format
                .compress(view(&b"chunked output ".repeat(5_000)), memory.clone())
                .expect("compression succeeds");
            let mut decompressor = format.decompressor().output_chunk_size(bound).build(memory);
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            loop {
                match decompressor.pull().expect("pull succeeds") {
                    Output::Data(chunk) => {
                        assert!(chunk.len() <= bound.get(), "{format:?} produced a {} byte chunk", chunk.len());
                    }
                    Output::Progress => {}
                    Output::NeedInput => panic!("decompressor requested input after end"),
                    Output::Done => break,
                }
            }
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_trailing_data_policy() {
        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let compressed = format.compress(view(b"payload"), memory.clone()).expect("compression succeeds");
            let joined = BytesView::from_views([compressed, view(b"trailing")]);
            let mut decompressor = format
                .decompressor()
                .multi_stream(false)
                .trailing_data(TrailingData::Reject)
                .build(memory);
            decompressor.push(joined).expect("push succeeds");
            decompressor.end_input();

            let error = loop {
                match decompressor.pull() {
                    Ok(Output::Data(_) | Output::Progress) => {}
                    Ok(_) => panic!("{format:?}: trailing input unexpectedly completed"),
                    Err(error) => break error,
                }
            };

            assert!(error.is_corrupt_data(), "{format:?}: got {error}");
        }
    }

    #[test]
    fn explicit_limits_are_available_on_the_one_shot_runtime_api() {
        for &format in Format::ALL {
            let memory = GlobalPool::new();
            let compressed = format
                .compress(view(&vec![0_u8; 4096]), memory.clone())
                .expect("compression succeeds");
            let error = format
                .decompress_with_limits(
                    compressed,
                    memory,
                    DecompressionLimits::new().without_max_ratio().with_max_output_len(1024),
                )
                .expect_err("the explicit cap fires");

            assert!(error.is_limit_exceeded(), "{format:?}: got {error}");
        }
    }

    #[test]
    fn multi_stream_governs_every_format() {
        // The generic half of the contract: whatever the format, setting this explicitly decides
        // whether a second stream is decompressed or ignored.
        let memory = GlobalPool::new();
        let payload = b"member ".repeat(50);

        for &format in Format::ALL {
            let compressed = format.compress(view(&payload), memory.clone()).expect("compress");
            let joined = BytesView::from_views([compressed.clone(), compressed]);

            let joined_len = decompressed_len(format.decompressor().multi_stream(true).build(memory.clone()), joined.clone());
            assert_eq!(joined_len, payload.len() * 2, "{format:?} should join with multi_stream(true)");

            let single_len = decompressed_len(format.decompressor().multi_stream(false).build(memory.clone()), joined);
            assert_eq!(single_len, payload.len(), "{format:?} should stop with multi_stream(false)");
        }
    }

    #[test]
    fn each_format_keeps_its_own_multi_stream_default() {
        // The format-specific half: the runtime builder must preserve each format's own default
        // rather than flattening every format to one behaviour. Gzip and zstd join, matching
        // `gzip(1)` and the `zstd` tool; the rest stop at the first stream.
        let memory = GlobalPool::new();
        let payload = b"member ".repeat(50);

        for &format in Format::ALL {
            // Matching the variant by name keeps this free of the cfg gates the variants carry.
            let joins_by_default = matches!(format!("{format:?}").as_str(), "Gzip" | "Zstd");

            let compressed = format.compress(view(&payload), memory.clone()).expect("compress");
            let joined = BytesView::from_views([compressed.clone(), compressed]);

            let len = decompressed_len(format.decompressor().build(memory.clone()), joined);
            let expected = if joins_by_default { payload.len() * 2 } else { payload.len() };

            assert_eq!(len, expected, "{format:?} did not keep its documented default");
        }
    }

    fn decompressed_len(decompressor: Box<dyn Decompressing>, input: BytesView) -> usize {
        decompressor.decompress(input).expect("decompression succeeds").len()
    }

    #[test]
    fn all_lists_exactly_the_compiled_in_formats() {
        let expected = usize::from(cfg!(feature = "deflate"))
            + usize::from(cfg!(feature = "zlib"))
            + usize::from(cfg!(feature = "gzip"))
            + usize::from(cfg!(feature = "brotli"))
            + usize::from(cfg!(feature = "zstd"));

        assert_eq!(Format::ALL.len(), expected);
    }
}
