// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Choosing a compression format at runtime.
//!
//! [`Format`] is the entry point, and the `build_format` methods on the shared builders live here
//! beside it, because this is the only place that has to know every format by name.

use bytesbuf::BytesView;

use crate::builder::{CompressorBuilder, DecompressorBuilder};
use crate::core::{Compress, Compression, Decompress};
use crate::error::{BuildError, Result};
use crate::limits::DecompressionLimits;
use crate::resources::Resources;

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
/// use compressors::Format;
/// use compressors::core::Compression;
/// use compressors::{CompressorBuilder, Level, Resources};
///
/// // The format arrives as a string, from an HTTP header.
/// let format = Format::from_content_encoding("gzip").expect("a supported encoding");
///
/// let memory = GlobalPool::new();
/// let mut compressor = CompressorBuilder::new()
///     .level(Level::HIGH)
///     .build_format(format, &Resources::default())?;
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

    /// Compresses a complete byte sequence that is already in memory.
    ///
    /// Uses [`Level::DEFAULT`][crate::Level::DEFAULT]; for anything else, configure a
    /// [`CompressorBuilder`] and finish it with
    /// [`build_format`][CompressorBuilder::build_format].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying compression engine fails.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "one-shot operations consistently borrow the selected runtime format"
    )]
    pub fn compress(&self, input: BytesView, resources: &Resources) -> Result<BytesView> {
        crate::compress(input, CompressorBuilder::new().build_format(*self, resources)?)
    }

    /// Decompresses a complete stream that is already in memory.
    ///
    /// Applies [`DecompressionLimits::new()`]; for anything else, configure a
    /// [`DecompressorBuilder`] and finish it with
    /// [`build_format`][DecompressorBuilder::build_format].
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed, truncated, or exceeds the default limits.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "one-shot operations consistently borrow the selected runtime format"
    )]
    pub fn decompress(&self, input: BytesView, resources: &Resources) -> Result<BytesView> {
        crate::decompress(input, DecompressorBuilder::new().build_format(*self, resources)?)
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
    pub fn decompress_with_limits(&self, input: BytesView, resources: &Resources, limits: DecompressionLimits) -> Result<BytesView> {
        crate::decompress(input, DecompressorBuilder::new().limits(limits).build_format(*self, resources)?)
    }
}

impl CompressorBuilder<()> {
    /// Builds a compressor for a format chosen at runtime.
    ///
    /// The result is boxed, because the concrete type is not known until `format` is. A boxed
    /// [`Compression`] is itself a `Compression`, so it fits anywhere a concrete compressor does.
    ///
    /// Everything this builder carries means the same thing in every format. A setting only one
    /// format has -- brotli's quality, say -- needs that format's own builder, whose result can be
    /// boxed to the same trait object.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`][crate::BuildError] if the chosen format's engine rejects the
    /// configuration.
    #[cfg_attr(
        not(any(feature = "brotli", feature = "zstd")),
        expect(
            clippy::unnecessary_wraps,
            reason = "brotli and zstd are the formats whose engines can reject a configuration, and neither is enabled"
        )
    )]
    pub fn build_format(
        self,
        format: Format,
        resources: &Resources,
    ) -> ::core::result::Result<Box<dyn Compression<Mode = Compress>>, BuildError> {
        Ok(match format {
            #[cfg(feature = "deflate")]
            Format::Deflate => Box::new(self.build_deflate(resources)),
            #[cfg(feature = "zlib")]
            Format::Zlib => Box::new(self.build_zlib(resources)),
            #[cfg(feature = "gzip")]
            Format::Gzip => Box::new(self.build_gzip(resources)),
            #[cfg(feature = "brotli")]
            Format::Brotli => Box::new(self.build_brotli(resources)?),
            #[cfg(feature = "zstd")]
            Format::Zstd => Box::new(self.build_zstd(resources)?),
        })
    }
}

impl DecompressorBuilder<()> {
    /// Builds a decompressor for a format chosen at runtime.
    ///
    /// The result is boxed, because the concrete type is not known until `format` is. A boxed
    /// [`Compression`] is itself a `Compression`, so it fits anywhere a concrete decompressor does.
    ///
    /// Bounds left unset on [`limits`][DecompressorBuilder::limits], and a
    /// [`multi_stream`][DecompressorBuilder::multi_stream] left unset, keep whatever the chosen
    /// format defaults to.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`][crate::BuildError] if the chosen format's engine rejects the
    /// configuration.
    #[cfg_attr(
        not(feature = "zstd"),
        expect(
            clippy::unnecessary_wraps,
            reason = "zstd is the only format whose decompressor engine can reject a configuration, and it is not enabled"
        )
    )]
    pub fn build_format(
        self,
        format: Format,
        resources: &Resources,
    ) -> ::core::result::Result<Box<dyn Compression<Mode = Decompress>>, BuildError> {
        Ok(match format {
            #[cfg(feature = "deflate")]
            Format::Deflate => Box::new(self.build_deflate(resources)),
            #[cfg(feature = "zlib")]
            Format::Zlib => Box::new(self.build_zlib(resources)),
            #[cfg(feature = "gzip")]
            Format::Gzip => Box::new(self.build_gzip(resources)),
            #[cfg(feature = "brotli")]
            Format::Brotli => Box::new(self.build_brotli(resources)),
            #[cfg(feature = "zstd")]
            Format::Zstd => Box::new(self.build_zstd(resources)?),
        })
    }
}
#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use bytesbuf::mem::GlobalPool;

    use super::*;
    use crate::level::Level;
    use crate::trailing::TrailingData;

    fn view(bytes: &[u8]) -> BytesView {
        BytesView::copied_from_slice(bytes, &GlobalPool::new())
    }

    fn compressed_len(builder: CompressorBuilder<()>, format: Format, payload: &[u8]) -> usize {
        let mut compressor = builder
            .build_format(format, &Resources::default())
            .expect("the settings are accepted");
        compressor.push(view(payload)).expect("push succeeds");
        compressor.end_input();

        let mut total = 0;
        loop {
            let output = compressor.pull().expect("pull succeeds");
            assert!(!output.is_need_input(), "compressor requested input after end");
            let done = output.is_done();
            if let Some(chunk) = output.into_data() {
                total += chunk.len();
            }
            if done {
                break;
            }
        }

        total
    }

    #[test]
    fn every_format_round_trips_through_the_enum() {
        let payload = b"runtime selected format ".repeat(200);

        for &format in Format::ALL {
            let compressed = format
                .compress(view(&payload), &Resources::default())
                .expect("compression succeeds");
            let plain = format
                .decompress(compressed, &Resources::default())
                .expect("decompression succeeds");

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
            let fast = compressed_len(CompressorBuilder::new().level(Level::FAST), format, &payload);
            let best = compressed_len(CompressorBuilder::new().level(Level::HIGH), format, &payload);

            assert!(best <= fast, "{format:?}: best={best} should not exceed fast={fast}");
        }
    }

    #[test]
    fn the_compressor_builder_applies_its_chunk_size() {
        let bound = NonZeroUsize::new(128).expect("128 is not zero");

        for &format in Format::ALL {
            let mut compressor = CompressorBuilder::new()
                .output_chunk_size(bound)
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            compressor.push(view(&b"chunked ".repeat(5_000))).expect("push succeeds");
            compressor.end_input();

            loop {
                let output = compressor.pull().expect("pull succeeds");
                assert!(!output.is_need_input(), "compressor requested input after end");
                let done = output.is_done();
                if let Some(chunk) = output.as_data() {
                    assert!(chunk.len() <= bound.get(), "{format:?} produced a {} byte chunk", chunk.len());
                }
                if done {
                    break;
                }
            }
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_limits() {
        for &format in Format::ALL {
            let compressed = format
                .compress(view(&vec![0_u8; 4 * 1024 * 1024]), &Resources::default())
                .expect("compression succeeds");

            let mut decompressor = DecompressorBuilder::new()
                .limits(DecompressionLimits::new().without_max_ratio().with_max_output_len(1024))
                .output_chunk_size(NonZeroUsize::new(64).expect("64 is not zero"))
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            let error = loop {
                match decompressor.pull() {
                    Ok(output) => {
                        assert!(
                            !output.is_done() && !output.is_need_input(),
                            "{format:?}: the cap should have fired"
                        );
                    }
                    Err(error) => break error,
                }
            };

            assert!(error.is_limit_exceeded(), "{format:?}: got {error}");
        }
    }

    #[test]
    fn a_decompressor_without_an_explicit_chunk_size_defaults_to_64_kib() {
        // Hardcoded literal (rather than `DEFAULT_CHUNK_SIZE`) so this test pins the actual byte
        // count instead of trivially matching whatever the constant happens to be set to.
        const EXPECTED_DEFAULT_CHUNK_SIZE: usize = 65_536;

        for &format in Format::ALL {
            let compressed = format
                .compress(view(&vec![0_u8; 4 * 1024 * 1024]), &Resources::default())
                .expect("compression succeeds");

            let mut decompressor = DecompressorBuilder::new()
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            let mut saw_a_full_size_chunk = false;
            loop {
                let output = decompressor.pull().expect("pull succeeds");
                assert!(!output.is_need_input(), "decompressor requested input after end");
                let done = output.is_done();
                if let Some(chunk) = output.as_data() {
                    let chunk_len = chunk.len();
                    assert!(
                        chunk_len <= EXPECTED_DEFAULT_CHUNK_SIZE,
                        "{format:?} produced a {chunk_len} byte chunk, larger than the 64 KiB default"
                    );
                    saw_a_full_size_chunk |= chunk_len == EXPECTED_DEFAULT_CHUNK_SIZE;
                }
                if done {
                    break;
                }
            }

            assert!(
                saw_a_full_size_chunk,
                "{format:?}: decompressing 4 MiB of zeros never produced a full 64 KiB chunk"
            );
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_chunk_size() {
        let bound = NonZeroUsize::new(128).expect("128 is not zero");

        for &format in Format::ALL {
            let compressed = format
                .compress(view(&b"chunked output ".repeat(5_000)), &Resources::default())
                .expect("compression succeeds");
            let mut decompressor = DecompressorBuilder::new()
                .output_chunk_size(bound)
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            loop {
                let output = decompressor.pull().expect("pull succeeds");
                assert!(!output.is_need_input(), "decompressor requested input after end");
                let done = output.is_done();
                if let Some(chunk) = output.as_data() {
                    assert!(chunk.len() <= bound.get(), "{format:?} produced a {} byte chunk", chunk.len());
                }
                if done {
                    break;
                }
            }
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_trailing_data_policy() {
        for &format in Format::ALL {
            let compressed = format
                .compress(view(&b"payload ".repeat(4_096)), &Resources::default())
                .expect("compression succeeds");
            let joined = BytesView::from_views([compressed, view(b"trailing")]);
            let mut decompressor = DecompressorBuilder::new()
                .multi_stream(false)
                .trailing_data(TrailingData::Reject)
                .output_chunk_size(NonZeroUsize::new(64).expect("64 is not zero"))
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(joined).expect("push succeeds");
            decompressor.end_input();

            let error = loop {
                match decompressor.pull() {
                    Ok(output) => {
                        assert!(
                            !output.is_done() && !output.is_need_input(),
                            "{format:?}: trailing input unexpectedly completed"
                        );
                    }
                    Err(error) => break error,
                }
            };

            assert!(error.is_corrupt_data(), "{format:?}: got {error}");
        }
    }

    #[test]
    fn explicit_limits_are_available_on_the_one_shot_runtime_api() {
        for &format in Format::ALL {
            let compressed = format
                .compress(view(&vec![0_u8; 4096]), &Resources::default())
                .expect("compression succeeds");
            let error = format
                .decompress_with_limits(
                    compressed,
                    &Resources::default(),
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
        let payload = b"member ".repeat(50);

        for &format in Format::ALL {
            let compressed = format.compress(view(&payload), &Resources::default()).expect("compress");
            let joined = BytesView::from_views([compressed.clone(), compressed]);

            let joined_len = decompressed_len(
                decompressor_for(DecompressorBuilder::new().multi_stream(true), format),
                joined.clone(),
            );
            assert_eq!(joined_len, payload.len() * 2, "{format:?} should join with multi_stream(true)");

            let single_len = decompressed_len(decompressor_for(DecompressorBuilder::new().multi_stream(false), format), joined);
            assert_eq!(single_len, payload.len(), "{format:?} should stop with multi_stream(false)");
        }
    }

    #[test]
    fn each_format_keeps_its_own_multi_stream_default() {
        // The format-specific half: the runtime builder must preserve each format's own default
        // rather than flattening every format to one behaviour. Gzip and zstd join, matching
        // `gzip(1)` and the `zstd` tool; the rest stop at the first stream.
        let payload = b"member ".repeat(50);

        for &format in Format::ALL {
            // Matching the variant by name keeps this free of the cfg gates the variants carry.
            let joins_by_default = matches!(format!("{format:?}").as_str(), "Gzip" | "Zstd");

            let compressed = format.compress(view(&payload), &Resources::default()).expect("compress");
            let joined = BytesView::from_views([compressed.clone(), compressed]);

            let len = decompressed_len(decompressor_for(DecompressorBuilder::new(), format), joined);
            let expected = if joins_by_default { payload.len() * 2 } else { payload.len() };

            assert_eq!(len, expected, "{format:?} did not keep its documented default");
        }
    }

    fn decompressed_len(decompressor: Box<dyn Compression<Mode = Decompress>>, input: BytesView) -> usize {
        crate::decompress(input, decompressor).expect("decompression succeeds").len()
    }

    fn decompressor_for(builder: DecompressorBuilder<()>, format: Format) -> Box<dyn Compression<Mode = Decompress>> {
        builder
            .build_format(format, &Resources::default())
            .expect("the settings are accepted")
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
