// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Choosing a compression format at runtime.
//!
//! [`Format`] is the entry point, and this module carries the same shape every format module does
//! -- a [`Compressor`], a [`Decompressor`], and the [`compress`], [`decompress`] and
//! [`decompress_with_limits`] conveniences -- with the format threaded through at runtime rather
//! than fixed at compile time. The `build_format` methods on the shared builders live here too,
//! because this is the only place that has to know every format by name.

use bytesbuf::BytesView;

use crate::builder::{CompressorBuilder, DecompressorBuilder};
use crate::core::{Compress, Compression, CompressionInternal, Decompress, Output};
use crate::error::{BuildError, Result};
use crate::limits::DecompressorLimits;
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
/// # #[cfg(feature = "gzip")]
/// # {
/// use compressors::format::{self, Format};
/// use compressors::{CompressorBuilder, Level, Resources};
///
/// // The format arrives as a string, from an HTTP header.
/// let format = Format::from_content_encoding("gzip").expect("a supported encoding");
///
/// let resources = Resources::global();
/// let compressed = format::compress(format, b"payload", resources)?;
///
/// assert_eq!(compressed.range(0..2).to_vec(), vec![0x1f, 0x8b]);
///
/// // Or build the compressor yourself when the level or chunk size matters.
/// let tuned = CompressorBuilder::new()
///     .level(Level::HIGH)
///     .build_format(format, resources)?;
/// # let _ = tuned;
/// # }
/// # Ok::<(), compressors::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// Raw deflate, RFC 1951. See `deflate`. Requires the `deflate` feature.
    #[cfg(any(test, feature = "deflate"))]
    Deflate,
    /// Zlib, RFC 1950. See `zlib`. Requires the `zlib` feature.
    #[cfg(any(test, feature = "zlib"))]
    Zlib,
    /// Gzip, RFC 1952. See `gzip`. Requires the `gzip` feature.
    #[cfg(any(test, feature = "gzip"))]
    Gzip,
    /// Brotli, RFC 7932. See `brotli`. Requires the `brotli` feature.
    #[cfg(any(test, feature = "brotli"))]
    Brotli,
    /// Zstandard, RFC 8878. See `zstd`. Requires the `zstd` feature.
    #[cfg(any(test, feature = "zstd"))]
    Zstd,
}

impl Format {
    /// Every format this build supports, in no particular order.
    ///
    /// The contents depend on which cargo features are enabled.
    pub const ALL: &'static [Self] = &[
        #[cfg(any(test, feature = "deflate"))]
        Self::Deflate,
        #[cfg(any(test, feature = "zlib"))]
        Self::Zlib,
        #[cfg(any(test, feature = "gzip"))]
        Self::Gzip,
        #[cfg(any(test, feature = "brotli"))]
        Self::Brotli,
        #[cfg(any(test, feature = "zstd"))]
        Self::Zstd,
    ];

    /// The HTTP `Content-Encoding` token for this format, if it has one.
    ///
    /// Returns `None` for `Format::Deflate`: raw deflate has no HTTP token. Note that the HTTP
    /// `deflate` token means a *zlib* stream, not raw deflate, so it maps to `Format::Zlib`.
    #[must_use]
    #[cfg_attr(
        all(
            not(any(test, feature = "deflate")),
            any(feature = "brotli", feature = "gzip", feature = "zlib", feature = "zstd")
        ),
        expect(
            clippy::unnecessary_wraps,
            reason = "raw deflate is the only format without an HTTP token, and it is not enabled in this configuration"
        )
    )]
    pub const fn content_encoding(self) -> Option<&'static str> {
        match self {
            #[cfg(any(test, feature = "deflate"))]
            Self::Deflate => None,
            #[cfg(any(test, feature = "zlib"))]
            Self::Zlib => Some("deflate"),
            #[cfg(any(test, feature = "gzip"))]
            Self::Gzip => Some("gzip"),
            #[cfg(any(test, feature = "brotli"))]
            Self::Brotli => Some("br"),
            #[cfg(any(test, feature = "zstd"))]
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

        #[cfg(any(test, feature = "gzip"))]
        if token.eq_ignore_ascii_case("gzip") || token.eq_ignore_ascii_case("x-gzip") {
            return Some(Self::Gzip);
        }

        #[cfg(any(test, feature = "zlib"))]
        if token.eq_ignore_ascii_case("deflate") {
            return Some(Self::Zlib);
        }

        #[cfg(any(test, feature = "brotli"))]
        if token.eq_ignore_ascii_case("br") {
            return Some(Self::Brotli);
        }

        #[cfg(any(test, feature = "zstd"))]
        if token.eq_ignore_ascii_case("zstd") {
            return Some(Self::Zstd);
        }

        #[cfg(not(any(test, feature = "brotli", feature = "gzip", feature = "zlib", feature = "zstd")))]
        let _ = token;

        None
    }
}

/// Dispatches one method to whichever format's engine a runtime-format compressor or decompressor is holding.
///
/// With no format feature enabled the enum has no variants, so this expands to a match on an
/// uninhabited value -- which is exactly right: there is then no way to construct one.
macro_rules! dispatch {
    ($kind:ident, $value:expr, $codec:ident => $call:expr) => {
        match $value {
            #[cfg(any(test, feature = "deflate"))]
            $kind::Deflate($codec) => $call,
            #[cfg(any(test, feature = "zlib"))]
            $kind::Zlib($codec) => $call,
            #[cfg(any(test, feature = "gzip"))]
            $kind::Gzip($codec) => $call,
            #[cfg(any(test, feature = "brotli"))]
            $kind::Brotli($codec) => $call,
            #[cfg(any(test, feature = "zstd"))]
            $kind::Zstd($codec) => $call,
            #[cfg(not(any(
                test,
                feature = "brotli",
                feature = "deflate",
                feature = "gzip",
                feature = "zlib",
                feature = "zstd"
            )))]
            #[expect(
                clippy::uninhabited_references,
                reason = "the variant cannot be constructed, so a reference to it cannot exist for this arm to reach"
            )]
            $kind::Impossible(never) => match *never {},
        }
    };
}

#[derive(Debug)]
enum CompressorKind {
    #[cfg(any(test, feature = "deflate"))]
    Deflate(crate::deflate::Compressor),
    #[cfg(any(test, feature = "zlib"))]
    Zlib(crate::zlib::Compressor),
    #[cfg(any(test, feature = "gzip"))]
    Gzip(crate::gzip::Compressor),
    #[cfg(any(test, feature = "brotli"))]
    // Brotli's engine state dwarfs the others (roughly 6.5 KiB against 1 KiB), and this enum is
    // returned by value, so the odd one out is boxed to keep the common cases cheap to move.
    Brotli(Box<crate::brotli::Compressor>),
    #[cfg(any(test, feature = "zstd"))]
    Zstd(crate::zstd::Compressor),
    /// Keeps the dispatch below exhaustive when no format is enabled.
    ///
    /// [`Infallible`][core::convert::Infallible] cannot be constructed, so neither can this: the
    /// type exists so a build with no format can still name it, not so it can be used.
    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(dead_code, reason = "the placeholder exists to be matched, never constructed")
    )]
    #[cfg(not(any(
        test,
        feature = "brotli",
        feature = "deflate",
        feature = "gzip",
        feature = "zlib",
        feature = "zstd"
    )))]
    Impossible(core::convert::Infallible),
}

#[derive(Debug)]
enum DecompressorKind {
    #[cfg(any(test, feature = "deflate"))]
    Deflate(crate::deflate::Decompressor),
    #[cfg(any(test, feature = "zlib"))]
    Zlib(crate::zlib::Decompressor),
    #[cfg(any(test, feature = "gzip"))]
    Gzip(crate::gzip::Decompressor),
    #[cfg(any(test, feature = "brotli"))]
    // Boxed for the same reason as the compressor above.
    Brotli(Box<crate::brotli::Decompressor>),
    #[cfg(any(test, feature = "zstd"))]
    Zstd(crate::zstd::Decompressor),
    /// Keeps the dispatch exhaustive when no format is enabled, exactly as for the compressor above.
    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(dead_code, reason = "the placeholder exists to be matched, never constructed")
    )]
    #[cfg(not(any(
        test,
        feature = "brotli",
        feature = "deflate",
        feature = "gzip",
        feature = "zlib",
        feature = "zstd"
    )))]
    Impossible(core::convert::Infallible),
}

/// Compresses a stream of byte sequences into a format chosen at runtime.
///
/// The runtime-format counterpart of each format module's `Compressor`, and driven exactly the same
/// way -- through [`Compression`]. The chosen format is held internally,
/// so this is a concrete type rather than a trait object: it can be stored in a struct, returned
/// from a function and handed to [`compress`][crate::compress] like any other compressor.
///
/// Reach it through [`CompressorBuilder::build_format`], or [`compress`] for a complete buffer.
#[derive(Debug)]
pub struct Compressor {
    kind: CompressorKind,
}

impl Compressor {
    /// Creates a compressor for `format` at [`Level::DEFAULT`][crate::Level::DEFAULT].
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`] if the chosen format's engine rejects the
    /// default configuration, which in practice it never does.
    pub fn new(format: Format, resources: &Resources) -> ::core::result::Result<Self, BuildError> {
        Self::builder().build_format(format, resources)
    }

    /// Starts configuring a compressor whose format is chosen when it is built.
    #[must_use]
    pub fn builder() -> CompressorBuilder<()> {
        CompressorBuilder::new()
    }
}

impl Compression for Compressor {
    type Mode = Compress;
}

impl CompressionInternal for Compressor {
    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(unused_variables, reason = "the dispatch below diverges when no format is enabled")
    )]
    fn push(&mut self, input: BytesView) -> Result<()> {
        dispatch!(CompressorKind, &mut self.kind, codec => codec.push(input))
    }

    fn end_input(&mut self) {
        dispatch!(CompressorKind, &mut self.kind, codec => codec.end_input());
    }

    fn pull(&mut self) -> Result<Output> {
        dispatch!(CompressorKind, &mut self.kind, codec => codec.pull())
    }

    fn total_in(&self) -> u64 {
        dispatch!(CompressorKind, &self.kind, codec => codec.total_in())
    }

    fn total_out(&self) -> u64 {
        dispatch!(CompressorKind, &self.kind, codec => codec.total_out())
    }

    fn flush(&mut self) -> Result<()> {
        dispatch!(CompressorKind, &mut self.kind, codec => codec.flush())
    }
}

/// Decompresses a stream in a format chosen at runtime.
///
/// The runtime-format counterpart of each format module's `Decompressor`.
///
/// # Security
///
/// This carries whatever bounds it was built with and adds none of its own, exactly like every
/// other format's decompressor. [`decompress`] is the bounded convenience.
///
/// Reach it through [`DecompressorBuilder::build_format`], or [`decompress`] for a complete stream.
#[derive(Debug)]
pub struct Decompressor {
    kind: DecompressorKind,
}

impl Decompressor {
    /// Creates a decompressor for `format` with that format's own default bounds.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`] if the chosen format's engine rejects the
    /// default configuration, which in practice it never does.
    pub fn new(format: Format, resources: &Resources) -> ::core::result::Result<Self, BuildError> {
        Self::builder().build_format(format, resources)
    }

    /// Starts configuring a decompressor whose format is chosen when it is built.
    #[must_use]
    pub fn builder() -> DecompressorBuilder<()> {
        DecompressorBuilder::new()
    }
}

impl Compression for Decompressor {
    type Mode = Decompress;
}

impl CompressionInternal for Decompressor {
    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(unused_variables, reason = "the dispatch below diverges when no format is enabled")
    )]
    fn push(&mut self, input: BytesView) -> Result<()> {
        dispatch!(DecompressorKind, &mut self.kind, codec => codec.push(input))
    }

    fn end_input(&mut self) {
        dispatch!(DecompressorKind, &mut self.kind, codec => codec.end_input());
    }

    fn pull(&mut self) -> Result<Output> {
        dispatch!(DecompressorKind, &mut self.kind, codec => codec.pull())
    }

    fn total_in(&self) -> u64 {
        dispatch!(DecompressorKind, &self.kind, codec => codec.total_in())
    }

    fn total_out(&self) -> u64 {
        dispatch!(DecompressorKind, &self.kind, codec => codec.total_out())
    }

    fn flush(&mut self) -> Result<()> {
        dispatch!(DecompressorKind, &mut self.kind, codec => codec.flush())
    }
}

/// Compresses a complete byte sequence into `format`.
///
/// Uses [`Level::DEFAULT`][crate::Level::DEFAULT]; for anything else, configure a
/// [`CompressorBuilder`] and finish it with [`build_format`][CompressorBuilder::build_format].
/// Prefer [`Compressor`] for data that arrives incrementally; this convenience buffers the entire
/// result before returning.
///
/// # Errors
///
/// Returns an error if the underlying compression engine fails.
pub fn compress(format: Format, input: impl crate::InputData, resources: &Resources) -> Result<BytesView> {
    let input = crate::InputData::into_view(input, resources);

    crate::compress(input, Compressor::new(format, resources)?)
}

/// Decompresses a complete stream in `format`.
///
/// Applies that format's default bounds plus this convenience's buffering caps, because it
/// accumulates the whole result. Prefer [`Decompressor`] for data that arrives incrementally.
///
/// # Errors
///
/// Returns an error if the data is malformed, truncated, or exceeds those bounds.
pub fn decompress(format: Format, input: impl crate::InputData, resources: &Resources) -> Result<BytesView> {
    decompress_with_limits(format, input, resources, DecompressorLimits::new())
}

/// Decompresses a complete stream in `format` with explicit output limits.
///
/// # Errors
///
/// Returns an error if the data is malformed, truncated, or exceeds `limits`. Bounds left unset on
/// `limits` still receive this convenience's buffering caps.
pub fn decompress_with_limits(
    format: Format,
    input: impl crate::InputData,
    resources: &Resources,
    limits: DecompressorLimits,
) -> Result<BytesView> {
    let input = crate::InputData::into_view(input, resources);

    crate::decompress(
        input,
        Decompressor::builder()
            .limits(limits.for_buffered_output())
            .build_format(format, resources)?,
    )
}

impl CompressorBuilder<()> {
    /// Builds a compressor for a format chosen at runtime.
    ///
    /// The result is this module's [`Compressor`], a concrete type like every other format's, so it
    /// fits anywhere one of those does. The chosen format is an implementation detail of the value
    /// rather than something the caller has to name.
    ///
    /// Everything this builder carries means the same thing in every format. A setting only one
    /// format has -- brotli's quality, say -- needs that format's own builder.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`] if the chosen format's engine rejects the
    /// configuration.
    #[cfg_attr(
        not(any(test, feature = "brotli", feature = "zstd")),
        expect(
            clippy::unnecessary_wraps,
            reason = "brotli and zstd are the formats whose engines can reject a configuration, and neither is enabled"
        )
    )]
    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(
            unreachable_code,
            unused_variables,
            clippy::unused_self,
            reason = "with no format enabled `Format` has no variants, so the match below diverges"
        )
    )]
    pub fn build_format(self, format: Format, resources: &Resources) -> ::core::result::Result<Compressor, BuildError> {
        let kind = match format {
            #[cfg(any(test, feature = "deflate"))]
            Format::Deflate => CompressorKind::Deflate(self.build_deflate(resources)),
            #[cfg(any(test, feature = "zlib"))]
            Format::Zlib => CompressorKind::Zlib(self.build_zlib(resources)),
            #[cfg(any(test, feature = "gzip"))]
            Format::Gzip => CompressorKind::Gzip(self.build_gzip(resources)),
            #[cfg(any(test, feature = "brotli"))]
            Format::Brotli => CompressorKind::Brotli(Box::new(self.build_brotli(resources)?)),
            #[cfg(any(test, feature = "zstd"))]
            Format::Zstd => CompressorKind::Zstd(self.build_zstd(resources)?),
        };

        Ok(Compressor { kind })
    }
}

impl DecompressorBuilder<()> {
    /// Builds a decompressor for a format chosen at runtime.
    ///
    /// The result is this module's [`Decompressor`], a concrete type like every other format's.
    ///
    /// Bounds left unset on [`limits`][DecompressorBuilder::limits], and a
    /// [`multi_stream`][DecompressorBuilder::multi_stream] left unset, keep whatever the chosen
    /// format defaults to.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`] if the chosen format's engine rejects the
    /// configuration.
    #[cfg_attr(
        not(feature = "zstd"),
        expect(
            clippy::unnecessary_wraps,
            reason = "zstd is the only format whose decompressor engine can reject a configuration, and it is not enabled"
        )
    )]
    #[cfg_attr(
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        )),
        expect(
            unreachable_code,
            unused_variables,
            clippy::unused_self,
            reason = "with no format enabled `Format` has no variants, so the match below diverges"
        )
    )]
    pub fn build_format(self, format: Format, resources: &Resources) -> ::core::result::Result<Decompressor, BuildError> {
        let kind = match format {
            #[cfg(any(test, feature = "deflate"))]
            Format::Deflate => DecompressorKind::Deflate(self.build_deflate(resources)),
            #[cfg(any(test, feature = "zlib"))]
            Format::Zlib => DecompressorKind::Zlib(self.build_zlib(resources)),
            #[cfg(any(test, feature = "gzip"))]
            Format::Gzip => DecompressorKind::Gzip(self.build_gzip(resources)),
            #[cfg(any(test, feature = "brotli"))]
            Format::Brotli => DecompressorKind::Brotli(Box::new(self.build_brotli(resources))),
            #[cfg(any(test, feature = "zstd"))]
            Format::Zstd => DecompressorKind::Zstd(self.build_zstd(resources)?),
        };

        Ok(Decompressor { kind })
    }
}
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::num::{NonZeroU64, NonZeroUsize};

    use bytesbuf::BytesBuf;

    use super::*;
    use crate::level::Level;
    use crate::limits::FormatLimits;
    use crate::testing::view;
    use crate::trailing::TrailingData;

    /// Caps every drain loop in this module.
    ///
    /// A conforming engine always terminates, so exceeding this means the code under test is
    /// spinning. A hanging test reports nothing at all, so the cap turns a hang into a failure --
    /// which also lets mutation testing reach a verdict instead of timing out.
    ///
    /// The cap has to stay tight enough for that verdict to arrive inside the mutation harness's
    /// per-mutant timeout. No test here needs more than a few hundred steps, so this leaves well
    /// over an order of magnitude of headroom while still failing a spinning mutant in under a
    /// second. Matches the cap the other drain-loop tests use.
    const MAX_STEPS: usize = 10_000;

    /// Fails a spinning test instead of letting it hang.
    ///
    /// A conforming engine always terminates, so exceeding the cap means the code under test is
    /// looping. A hanging test reports nothing at all, and mutation testing records a timeout rather
    /// than a verdict, so every drain loop here counts its steps through this.
    struct StepGuard(usize);

    impl StepGuard {
        fn new() -> Self {
            Self(0)
        }

        fn step(&mut self) {
            self.0 += 1;
            assert!(self.0 < MAX_STEPS, "the operation did not finish within {MAX_STEPS} steps");
        }
    }

    fn compressed_len(builder: CompressorBuilder<()>, format: Format, payload: &[u8]) -> usize {
        let mut compressor = builder
            .build_format(format, &Resources::default())
            .expect("the settings are accepted");
        compressor.push(view(payload)).expect("push succeeds");
        compressor.end_input();

        let mut total = 0;
        let mut finished = false;
        for _ in 0..MAX_STEPS {
            let output = compressor.pull().expect("pull succeeds");
            assert!(!output.is_need_input(), "compressor requested input after end");
            let done = output.is_done();
            if let Some(chunk) = output.into_data() {
                total += chunk.len();
            }
            if done {
                finished = true;
                break;
            }
        }
        assert!(finished, "compression did not finish within {MAX_STEPS} steps");

        total
    }

    #[test]
    fn the_runtime_format_codecs_report_their_byte_counters_and_flush() {
        // The counters and `flush` forward to whichever codec the enum is holding, so they need
        // driving for every format rather than only for whichever one happens to be first.
        let payload = b"counted and flushed ".repeat(200);

        for &format in Format::ALL {
            let mut compressor = Compressor::new(format, &Resources::default()).expect("the defaults are accepted");

            compressor.push(view(&payload)).expect("push succeeds");
            compressor.flush().expect("flush succeeds");

            let mut compressed = BytesBuf::new();
            let mut guard = 0;
            loop {
                guard += 1;
                assert!(guard < MAX_STEPS, "the flush did not settle for {format:?}");

                let output = compressor.pull().expect("pull succeeds");
                if output.is_need_input() {
                    break;
                }
                if let Some(chunk) = output.into_data() {
                    compressed.put_bytes(chunk);
                }
            }

            assert_eq!(
                compressor.total_in(),
                payload.len() as u64,
                "{format:?} miscounted what it consumed"
            );
            assert!(compressor.total_out() > 0, "{format:?} reported no output after a flush");

            compressor.end_input();
            loop {
                guard += 1;
                assert!(guard < MAX_STEPS, "compression did not finish for {format:?}");

                let output = compressor.pull().expect("pull succeeds");
                let done = output.is_done();
                if let Some(chunk) = output.into_data() {
                    compressed.put_bytes(chunk);
                }
                if done {
                    break;
                }
            }

            let mut decompressor = Decompressor::new(format, &Resources::default()).expect("the defaults are accepted");

            decompressor.push(compressed.consume_all()).expect("push succeeds");
            decompressor.flush().expect("a decompressor has nothing to flush");
            decompressor.end_input();

            let mut plain = BytesBuf::new();
            loop {
                guard += 1;
                assert!(guard < MAX_STEPS, "decompression did not finish for {format:?}");

                let output = decompressor.pull().expect("pull succeeds");
                let done = output.is_done();
                if let Some(chunk) = output.into_data() {
                    plain.put_bytes(chunk);
                }
                if done {
                    break;
                }
            }

            assert_eq!(plain.consume_all().to_vec(), payload, "{format:?} did not round trip");
            assert_eq!(
                decompressor.total_out(),
                payload.len() as u64,
                "{format:?} miscounted what it produced"
            );
            assert!(decompressor.total_in() > 0, "{format:?} reported consuming nothing");
        }
    }

    #[test]
    fn every_format_round_trips_through_the_enum() {
        let payload = b"runtime selected format ".repeat(200);

        for &format in Format::ALL {
            let compressed = crate::format::compress(format, view(&payload), &Resources::default()).expect("compression succeeds");
            let plain = crate::format::decompress(format, compressed, &Resources::default()).expect("decompression succeeds");

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

    #[cfg(any(test, all(feature = "deflate", feature = "zlib")))]
    #[test]
    fn http_deflate_token_means_zlib() {
        // The most common source of confusion in this area: the HTTP `deflate` token denotes a zlib
        // stream, not raw deflate.
        assert_eq!(Format::from_content_encoding("deflate"), Some(Format::Zlib));
        assert_eq!(Format::Deflate.content_encoding(), None);
    }

    #[cfg(any(test, feature = "gzip"))]
    #[test]
    fn content_encoding_parsing_is_case_insensitive_and_trims() {
        assert_eq!(Format::from_content_encoding("GZIP"), Some(Format::Gzip));
        assert_eq!(Format::from_content_encoding("  gzip  "), Some(Format::Gzip));
        assert_eq!(Format::from_content_encoding("x-gzip"), Some(Format::Gzip));
        assert_eq!(Format::from_content_encoding("identity"), None);
        assert_eq!(Format::from_content_encoding(""), None);
    }

    #[cfg(any(test, feature = "brotli"))]
    #[test]
    fn brotli_uses_the_br_token() {
        assert_eq!(Format::from_content_encoding("br"), Some(Format::Brotli));
        assert_eq!(Format::Brotli.content_encoding(), Some("br"));
    }

    #[test]
    fn unknown_tokens_are_rejected() {
        // A token no format claims, and one that names a format this crate deliberately gives no
        // content coding: raw deflate has none, because the HTTP `deflate` token means zlib.
        assert_eq!(Format::from_content_encoding("compress"), None);
        assert_eq!(Format::from_content_encoding("identity"), None);
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

            let mut finished = false;
            for _ in 0..MAX_STEPS {
                let output = compressor.pull().expect("pull succeeds");
                assert!(!output.is_need_input(), "compressor requested input after end");
                let done = output.is_done();
                if let Some(chunk) = output.as_data() {
                    assert!(chunk.len() <= bound.get(), "{format:?} produced a {} byte chunk", chunk.len());
                }
                if done {
                    finished = true;
                    break;
                }
            }
            assert!(finished, "{format:?} compression did not finish within {MAX_STEPS} steps");
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_limits() {
        for &format in Format::ALL {
            let compressed =
                crate::format::compress(format, view(&vec![0_u8; 256 * 1024]), &Resources::default()).expect("compression succeeds");

            let mut decompressor = DecompressorBuilder::new()
                .limits(
                    DecompressorLimits::new()
                        .without_max_ratio()
                        .with_max_output_len(NonZeroU64::new(1024).unwrap()),
                )
                .output_chunk_size(NonZeroUsize::new(64).expect("64 is not zero"))
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            let mut guard = StepGuard::new();
            let error = loop {
                guard.step();
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
            let compressed =
                crate::format::compress(format, view(&vec![0_u8; 256 * 1024]), &Resources::default()).expect("compression succeeds");

            let mut decompressor = DecompressorBuilder::new()
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            let mut saw_a_full_size_chunk = false;
            let mut finished = false;
            for _ in 0..MAX_STEPS {
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
                    finished = true;
                    break;
                }
            }
            assert!(finished, "{format:?} decompression did not finish within {MAX_STEPS} steps");

            assert!(
                saw_a_full_size_chunk,
                "{format:?}: decompressing a quarter megabyte of zeros never produced a full 64 KiB chunk"
            );
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_chunk_size() {
        let bound = NonZeroUsize::new(128).expect("128 is not zero");

        for &format in Format::ALL {
            let compressed = crate::format::compress(format, view(&b"chunked output ".repeat(5_000)), &Resources::default())
                .expect("compression succeeds");
            let mut decompressor = DecompressorBuilder::new()
                .output_chunk_size(bound)
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(compressed).expect("push succeeds");
            decompressor.end_input();

            let mut finished = false;
            for _ in 0..MAX_STEPS {
                let output = decompressor.pull().expect("pull succeeds");
                assert!(!output.is_need_input(), "decompressor requested input after end");
                let done = output.is_done();
                if let Some(chunk) = output.as_data() {
                    assert!(chunk.len() <= bound.get(), "{format:?} produced a {} byte chunk", chunk.len());
                }
                if done {
                    finished = true;
                    break;
                }
            }
            assert!(finished, "{format:?} decompression did not finish within {MAX_STEPS} steps");
        }
    }

    #[test]
    fn the_decompressor_builder_applies_its_trailing_data_policy() {
        for &format in Format::ALL {
            let compressed =
                crate::format::compress(format, view(&b"payload ".repeat(4_096)), &Resources::default()).expect("compression succeeds");
            let joined = BytesView::from_views([compressed, view(b"trailing")]);
            let mut decompressor = DecompressorBuilder::new()
                .multi_stream(false)
                .trailing_data(TrailingData::Reject)
                .output_chunk_size(NonZeroUsize::new(64).expect("64 is not zero"))
                .build_format(format, &Resources::default())
                .expect("the settings are accepted");
            decompressor.push(joined).expect("push succeeds");
            decompressor.end_input();

            let mut guard = StepGuard::new();
            let error = loop {
                guard.step();
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
            let compressed = crate::format::compress(format, view(&vec![0_u8; 4096]), &Resources::default()).expect("compression succeeds");
            let error = crate::format::decompress_with_limits(
                format,
                compressed,
                &Resources::default(),
                DecompressorLimits::new()
                    .without_max_ratio()
                    .with_max_output_len(NonZeroU64::new(1024).unwrap()),
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
            let compressed = crate::format::compress(format, view(&payload), &Resources::default()).expect("compress");
            let joined = BytesView::from_views([compressed.clone(), compressed]);

            let joined_len = decompressed_len(
                decompressor_for(DecompressorBuilder::new().multi_stream(true), format),
                joined.clone(),
            );
            assert_eq!(joined_len, payload.len() * 2, "{format:?} should join with multi_stream(true)");

            let single_len = decompressed_len(
                decompressor_for(
                    DecompressorBuilder::new().multi_stream(false).trailing_data(TrailingData::Ignore),
                    format,
                ),
                joined,
            );
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

            let compressed = crate::format::compress(format, view(&payload), &Resources::default()).expect("compress");
            let joined = BytesView::from_views([compressed.clone(), compressed]);

            // `Ignore` isolates the multi-stream default under test from the trailing-data policy,
            // which would otherwise reject the second member for the formats that do not join.
            let len = decompressed_len(
                decompressor_for(DecompressorBuilder::new().trailing_data(TrailingData::Ignore), format),
                joined,
            );
            let expected = if joins_by_default { payload.len() * 2 } else { payload.len() };

            assert_eq!(len, expected, "{format:?} did not keep its documented default");
        }
    }

    fn decompressed_len(decompressor: Decompressor, input: BytesView) -> usize {
        crate::decompress(input, decompressor).expect("decompression succeeds").len()
    }

    fn decompressor_for(builder: DecompressorBuilder<()>, format: Format) -> Decompressor {
        builder
            .build_format(format, &Resources::default())
            .expect("the settings are accepted")
    }

    #[test]
    fn no_format_bounds_output_or_stream_count_by_default() {
        // A decompressor hands each chunk straight back and keeps nothing, so bounding its total
        // output or member count would cut off long streams that never buffer more than one chunk.
        // Cumulative bounds belong to the conveniences that accumulate, applied by
        // `DecompressorLimits::for_buffered_output` and asserted in `limits`.
        fn assert_uncapped(name: &str, limits: FormatLimits) {
            const HUGE: u64 = 64 * 1024 * 1024 * 1024;

            // Input matches output so the ratio guard, which differs per format, never fires here.
            assert!(limits.check(HUGE, HUGE, 1).is_ok(), "{name} should not cap total output");
            assert!(limits.check(1, 0, HUGE).is_ok(), "{name} should not cap stream count");
        }

        #[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
        assert_uncapped("flate", crate::flate::DEFAULT_LIMITS);
        #[cfg(any(test, feature = "brotli"))]
        assert_uncapped("brotli", crate::brotli::DEFAULT_LIMITS);
        #[cfg(any(test, feature = "zstd"))]
        assert_uncapped("zstd", crate::zstd::DEFAULT_LIMITS);
    }

    #[test]
    fn all_lists_exactly_the_compiled_in_formats() {
        let expected = usize::from(cfg!(any(test, feature = "deflate")))
            + usize::from(cfg!(any(test, feature = "zlib")))
            + usize::from(cfg!(any(test, feature = "gzip")))
            + usize::from(cfg!(any(test, feature = "brotli")))
            + usize::from(cfg!(any(test, feature = "zstd")));

        assert_eq!(Format::ALL.len(), expected);
    }
}
