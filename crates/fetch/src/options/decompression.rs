// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(
    test,
    feature = "compression-gzip",
    feature = "compression-deflate",
    feature = "compression-brotli",
    feature = "compression-zstd"
))]
use compressors::format::Format;

/// A compression format the client can decompress automatically.
///
/// Each variant exists only when its Cargo feature is enabled, so the set of
/// variants is exactly the set this build can handle - and a build with none of
/// them has no variants at all, links no compression implementation, and can only ask for an empty
/// set.
///
/// Pass to [`DecompressionOptions::formats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecompressionFormat {
    /// The `zstd` format (RFC 8878).
    #[cfg(any(test, feature = "compression-zstd"))]
    Zstd,

    /// The `br` format (RFC 7932).
    #[cfg(any(test, feature = "compression-brotli"))]
    Brotli,

    /// The `gzip` format (RFC 1952). The legacy `x-gzip` token also selects it.
    #[cfg(any(test, feature = "compression-gzip"))]
    Gzip,

    /// The `deflate` format: zlib-wrapped DEFLATE (RFC 1950).
    ///
    /// Despite the token, the `deflate` format is *not* raw DEFLATE (RFC 1951).
    #[cfg(any(test, feature = "compression-deflate"))]
    Deflate,
}

impl DecompressionFormat {
    /// Every format compiled into this build, most recommended first.
    ///
    /// This is the order advertised in `Accept-Encoding`, so a server that
    /// supports several picks the one earliest in the list:
    ///
    /// 1. `zstd` decompresses several times faster than the rest at a comparable
    ///    ratio, which is what a client pays for.
    /// 2. `br` compresses text the hardest, and is the most widely deployed of
    ///    the two modern compression formats.
    /// 3. `gzip` is the universally understood fallback.
    /// 4. `deflate` is last: servers disagree over whether it means the
    ///    zlib-wrapped form the specification requires or raw DEFLATE, so it is
    ///    worth accepting but never worth preferring.
    pub const ALL: &'static [Self] = &[
        #[cfg(any(test, feature = "compression-zstd"))]
        Self::Zstd,
        #[cfg(any(test, feature = "compression-brotli"))]
        Self::Brotli,
        #[cfg(any(test, feature = "compression-gzip"))]
        Self::Gzip,
        #[cfg(any(test, feature = "compression-deflate"))]
        Self::Deflate,
    ];

    /// The corresponding compression format in `compressors`.
    #[cfg(any(
        test,
        feature = "compression-gzip",
        feature = "compression-deflate",
        feature = "compression-brotli",
        feature = "compression-zstd"
    ))]
    pub(crate) const fn format(self) -> Format {
        match self {
            #[cfg(any(test, feature = "compression-zstd"))]
            Self::Zstd => Format::Zstd,
            #[cfg(any(test, feature = "compression-brotli"))]
            Self::Brotli => Format::Brotli,
            #[cfg(any(test, feature = "compression-gzip"))]
            Self::Gzip => Format::Gzip,
            // HTTP's `deflate` is zlib-wrapped, so it maps to `Zlib`, not to the
            // raw-DEFLATE `Format::Deflate`, which has no HTTP token at all.
            #[cfg(any(test, feature = "compression-deflate"))]
            Self::Deflate => Format::Zlib,
        }
    }
}

/// Configures automatic response decompression and its resource limits.
///
/// Decompression is off until [`formats`][Self::formats] names a format. The
/// output limit inherits the selected compression format's default unless
/// explicitly configured here. The stream count defaults to 1,024 so empty
/// concatenated members cannot consume unbounded CPU.
/// Configured bounds apply while reading the body, including when streaming
/// without buffering it.
///
/// Pass to [`HttpClientBuilder::decompression`][crate::HttpClientBuilder::decompression].
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "compression-gzip")] {
/// use fetch::options::{DecompressionFormat, DecompressionOptions};
///
/// let options = DecompressionOptions::with_formats(&[DecompressionFormat::Gzip])
///     .max_output_len(8 * 1024 * 1024)
///     .max_streams(32);
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecompressionOptions {
    pub(crate) formats: Vec<DecompressionFormat>,
    output_limit: OutputLimit,
    stream_limit: StreamLimit,
}

/// HTTP responses should not contain enough members for this guardrail to be observable.
const DEFAULT_MAX_STREAMS: u64 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputLimit {
    Inherit,
    Bounded(u64),
    Unbounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamLimit {
    Bounded(u64),
    Unbounded,
}

impl DecompressionOptions {
    /// Creates options with no enabled formats, an inherited output limit, and
    /// the default 1,024-stream bound.
    ///
    /// This is what [`Default`] returns.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            formats: Vec::new(),
            output_limit: OutputLimit::Inherit,
            stream_limit: StreamLimit::Bounded(DEFAULT_MAX_STREAMS),
        }
    }

    /// Creates options that enable `formats` and use the default limits.
    #[must_use]
    pub fn with_formats(formats: &[DecompressionFormat]) -> Self {
        Self::new().formats(formats)
    }

    /// Decompresses responses using any of `formats`, most preferred first.
    ///
    /// Requests advertise these formats in `Accept-Encoding` unless the caller
    /// supplies that header or makes a range request. An empty list disables both
    /// automatic decompression and advertisement.
    ///
    /// [`DecompressionFormat::ALL`] selects every format compiled into this build.
    #[must_use]
    pub fn formats(mut self, formats: &[DecompressionFormat]) -> Self {
        self.formats = formats.to_vec();
        self
    }

    /// Bounds the total decompressed output, in bytes.
    ///
    /// Inherits the compression format's limit when this setter is not called. Pass a byte count
    /// or `Some(count)` to set a bound, or `None` to explicitly remove the output-size
    /// bound. An explicit bound applies even when the caller streams the body into
    /// a file or another service without buffering it in memory.
    ///
    /// This is independent of [`HttpBodyOptions::buffer_limit`][super::HttpBodyOptions::buffer_limit],
    /// which limits how much a body-buffering operation may retain.
    ///
    /// # Panics
    ///
    /// Panics if the supplied byte count is zero, including `Some(0)`.
    #[must_use]
    pub fn max_output_len(mut self, bytes: impl Into<Option<u64>>) -> Self {
        self.output_limit = match bytes.into() {
            Some(bytes) => {
                assert!(bytes > 0, "max_output_len must be greater than zero");
                OutputLimit::Bounded(bytes)
            }
            None => OutputLimit::Unbounded,
        };
        self
    }

    /// Bounds the number of compressed streams decoded by each decompression stage.
    ///
    /// Defaults to 1,024 streams. Each concatenated gzip member or zstd frame
    /// counts as one stream, including empty ones that would never reach the
    /// output-size bound.
    ///
    /// # Panics
    ///
    /// Panics if `streams` is zero.
    #[must_use]
    pub const fn max_streams(mut self, streams: u64) -> Self {
        assert!(streams > 0, "max_streams must be greater than zero");
        self.stream_limit = StreamLimit::Bounded(streams);
        self
    }

    /// Removes the stream-count bound.
    #[must_use]
    pub const fn unbounded_streams(mut self) -> Self {
        self.stream_limit = StreamLimit::Unbounded;
        self
    }

    #[cfg(any(
        test,
        feature = "compression-gzip",
        feature = "compression-deflate",
        feature = "compression-brotli",
        feature = "compression-zstd"
    ))]
    pub(crate) fn limits(&self) -> compressors::DecompressorLimits {
        use std::num::NonZeroU64;

        use compressors::DecompressorLimits;

        let limits = DecompressorLimits::new();
        let limits = match self.output_limit {
            OutputLimit::Inherit => limits,
            OutputLimit::Bounded(bytes) => {
                limits.max_output_len(NonZeroU64::new(bytes).expect("DecompressionOptions::max_output_len rejects zero"))
            }
            OutputLimit::Unbounded => limits.unbounded_output_len(),
        };

        match self.stream_limit {
            StreamLimit::Bounded(streams) => {
                limits.max_streams(NonZeroU64::new(streams).expect("DecompressionOptions::max_streams rejects zero"))
            }
            StreamLimit::Unbounded => limits.unbounded_streams(),
        }
    }
}

impl Default for DecompressionOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&[DecompressionFormat]> for DecompressionOptions {
    fn from(formats: &[DecompressionFormat]) -> Self {
        Self::with_formats(formats)
    }
}

impl<const N: usize> From<&[DecompressionFormat; N]> for DecompressionOptions {
    fn from(formats: &[DecompressionFormat; N]) -> Self {
        Self::from(formats.as_slice())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn http_deflate_is_zlib_wrapped_not_raw() {
        assert_eq!(DecompressionFormat::Deflate.format(), Format::Zlib);
        assert_eq!(Format::Zlib.content_encoding(), Some("deflate"));
    }

    #[test]
    fn every_format_maps_to_a_compressor_format_with_an_http_token() {
        for format in DecompressionFormat::ALL {
            assert!(format.format().content_encoding().is_some(), "format {format:?}");
        }
    }

    #[test]
    fn deflate_is_never_preferred() {
        // Servers disagree over whether `deflate` is zlib-wrapped or raw, so it
        // is worth accepting but must always rank below the alternatives.
        assert_eq!(DecompressionFormat::ALL.last().copied(), Some(DecompressionFormat::Deflate));
    }

    #[test]
    fn constructors_and_format_conversions_apply_only_the_default_stream_limit() {
        let expected = compressors::DecompressorLimits::new().max_streams(std::num::NonZeroU64::new(DEFAULT_MAX_STREAMS).unwrap());
        for options in [
            DecompressionOptions::new(),
            DecompressionOptions::default(),
            DecompressionOptions::with_formats(DecompressionFormat::ALL),
            DecompressionOptions::from(DecompressionFormat::ALL),
            DecompressionOptions::from(&[DecompressionFormat::Gzip]),
        ] {
            assert_eq!(options.limits(), expected);
        }
    }

    #[test]
    fn explicit_unbounded_output_is_distinct_from_inheritance() {
        let options = DecompressionOptions::new().max_output_len(None);
        assert_eq!(
            options.limits(),
            compressors::DecompressorLimits::new()
                .unbounded_output_len()
                .max_streams(std::num::NonZeroU64::new(DEFAULT_MAX_STREAMS).unwrap())
        );
    }

    #[test]
    fn stream_limit_can_be_overridden_or_removed() {
        let bounded = DecompressionOptions::new().max_streams(2);
        assert_eq!(
            bounded.limits(),
            compressors::DecompressorLimits::new().max_streams(std::num::NonZeroU64::new(2).unwrap())
        );

        let unbounded = DecompressionOptions::new().unbounded_streams();
        assert_eq!(unbounded.limits(), compressors::DecompressorLimits::new().unbounded_streams());
    }

    #[test]
    #[should_panic(expected = "max_output_len must be greater than zero")]
    fn zero_output_limit_is_a_configuration_error() {
        let _ = DecompressionOptions::new().max_output_len(0);
    }

    #[test]
    #[should_panic(expected = "max_output_len must be greater than zero")]
    fn an_optional_zero_output_limit_is_a_configuration_error() {
        let _ = DecompressionOptions::new().max_output_len(Some(0));
    }

    #[test]
    #[should_panic(expected = "max_streams must be greater than zero")]
    fn zero_stream_limit_is_a_configuration_error() {
        let _ = DecompressionOptions::new().max_streams(0);
    }
}
