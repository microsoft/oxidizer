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

/// A format that the client can decompress automatically.
///
/// A variant exists only when its Cargo feature is enabled. A build without a compression feature
/// has no variants and links no compression implementation.
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
    /// Every format in this build, in preference order.
    ///
    /// Requests use this order in `Accept-Encoding`:
    ///
    /// 1. `zstd` has the fastest decompression at a comparable compression ratio.
    /// 2. `br` has the best text compression and wider deployment than `zstd`.
    /// 3. `gzip` is the compatibility fallback.
    /// 4. `deflate` is last because servers disagree about whether it means zlib-wrapped or raw
    ///    DEFLATE.
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
            // HTTP `deflate` is zlib-wrapped. Raw `Format::Deflate` has no HTTP token.
            #[cfg(any(test, feature = "compression-deflate"))]
            Self::Deflate => Format::Zlib,
        }
    }
}

/// Configures automatic response decompression and its resource limits.
///
/// Decompression is off until [`formats`][Self::formats] selects a format. The client uses the
/// `http_compression` defaults: at most 64 MiB of output and 1,024 compressed streams for each
/// decompression stage. Configured limits apply while code reads the body, including a streamed
/// body.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputLimit {
    Inherit,
    Bounded(u64),
    Unbounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamLimit {
    Inherit,
    Bounded(u64),
    Unbounded,
}

impl DecompressionOptions {
    /// Creates options with no formats and the default decompression limits.
    ///
    /// This is what [`Default`] returns.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            formats: Vec::new(),
            output_limit: OutputLimit::Inherit,
            stream_limit: StreamLimit::Inherit,
        }
    }

    /// Creates options that enable `formats` and use the default limits.
    #[must_use]
    pub fn with_formats(formats: &[DecompressionFormat]) -> Self {
        Self::new().formats(formats)
    }

    /// Sets the formats in preference order.
    ///
    /// Requests list these formats in `Accept-Encoding` unless the caller supplies the header or
    /// makes a range request. An empty list disables decompression and the header.
    ///
    /// [`DecompressionFormat::ALL`] selects every format compiled into this build.
    #[must_use]
    pub fn formats(mut self, formats: &[DecompressionFormat]) -> Self {
        self.formats = formats.to_vec();
        self
    }

    /// Bounds the total decompressed output, in bytes.
    ///
    /// The `http_compression` default is currently 64 MiB. Pass a byte count or `Some(count)` to
    /// replace it. Pass `None` to remove the output limit. An explicit limit also applies when the
    /// caller streams the body without buffering it in memory.
    ///
    /// This limit is separate from
    /// [`HttpBodyOptions::buffer_limit`][super::HttpBodyOptions::buffer_limit], which limits memory
    /// retained while buffering a body.
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
    /// The `http_compression` default is currently 1,024 streams. Each concatenated gzip member or
    /// zstd frame counts as one stream, including an empty stream that cannot reach the output
    /// limit.
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
            StreamLimit::Inherit => limits,
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
        // Servers disagree about whether `deflate` is zlib-wrapped or raw, so rank it last.
        assert_eq!(DecompressionFormat::ALL.last().copied(), Some(DecompressionFormat::Deflate));
    }

    #[test]
    fn constructors_and_format_conversions_leave_http_defaults_unset() {
        let expected = compressors::DecompressorLimits::new();
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
        assert_eq!(options.limits(), compressors::DecompressorLimits::new().unbounded_output_len());
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
