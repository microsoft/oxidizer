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
/// them has no variants at all, links no codec, and can only ask for an empty
/// set.
///
/// Pass to [`ResponseDecompressionOptions::methods`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecompressionMethod {
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

impl DecompressionMethod {
    /// Every method compiled into this build, most recommended first.
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
/// Decompression is off until [`methods`][Self::methods] names a format. Limits
/// inherit the selected codec's defaults unless explicitly configured here.
/// Configured bounds apply while reading the body, including when streaming
/// without buffering it.
///
/// Pass to [`HttpClientBuilder::response_decompression`][crate::HttpClientBuilder::response_decompression].
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "compression-gzip")] {
/// use fetch::options::{DecompressionMethod, ResponseDecompressionOptions};
///
/// let options = ResponseDecompressionOptions::new()
///     .methods(&[DecompressionMethod::Gzip])
///     .max_output_len(8 * 1024 * 1024)
///     .max_streams(32);
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseDecompressionOptions {
    pub(crate) methods: Vec<DecompressionMethod>,
    output_limit: OutputLimit,
    max_streams: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputLimit {
    Inherit,
    Bounded(u64),
    Unbounded,
}

impl ResponseDecompressionOptions {
    /// Creates options with no enabled methods and no overrides of codec limits.
    ///
    /// This is what [`Default`] returns.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            methods: Vec::new(),
            output_limit: OutputLimit::Inherit,
            max_streams: None,
        }
    }

    /// Decompresses responses using any of `methods`, most preferred first.
    ///
    /// Requests advertise these methods in `Accept-Encoding` unless the caller
    /// supplies that header or makes a range request. An empty list disables both
    /// automatic decompression and advertisement.
    ///
    /// [`DecompressionMethod::ALL`] selects every method compiled into this build.
    #[must_use]
    pub fn methods(mut self, methods: &[DecompressionMethod]) -> Self {
        self.methods = methods.to_vec();
        self
    }

    /// Bounds the total decompressed output, in bytes.
    ///
    /// Inherits the codec's limit when this setter is not called. Pass a byte count
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
    /// Inherits the codec's limit by default. Each concatenated gzip member or zstd
    /// frame counts as one stream, including empty ones that would never reach the
    /// output-size bound.
    ///
    /// # Panics
    ///
    /// Panics if `streams` is zero.
    #[must_use]
    pub const fn max_streams(mut self, streams: u64) -> Self {
        assert!(streams > 0, "max_streams must be greater than zero");
        self.max_streams = Some(streams);
        self
    }

    #[cfg(any(
        test,
        feature = "compression-gzip",
        feature = "compression-deflate",
        feature = "compression-brotli",
        feature = "compression-zstd"
    ))]
    pub(crate) fn limits(&self) -> Option<compressors::DecompressorLimits> {
        use std::num::NonZeroU64;

        use compressors::DecompressorLimits;

        if self.output_limit == OutputLimit::Inherit && self.max_streams.is_none() {
            return None;
        }

        let limits = DecompressorLimits::new();
        let limits = match self.output_limit {
            OutputLimit::Inherit => limits,
            OutputLimit::Bounded(bytes) => {
                limits.max_output_len(NonZeroU64::new(bytes).expect("ResponseDecompressionOptions::max_output_len rejects zero"))
            }
            OutputLimit::Unbounded => limits.unbounded_output_len(),
        };

        Some(match self.max_streams {
            Some(streams) => limits.max_streams(NonZeroU64::new(streams).expect("ResponseDecompressionOptions::max_streams rejects zero")),
            None => limits,
        })
    }
}

impl Default for ResponseDecompressionOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&[DecompressionMethod]> for ResponseDecompressionOptions {
    fn from(methods: &[DecompressionMethod]) -> Self {
        Self::new().methods(methods)
    }
}

impl<const N: usize> From<&[DecompressionMethod; N]> for ResponseDecompressionOptions {
    fn from(methods: &[DecompressionMethod; N]) -> Self {
        Self::from(methods.as_slice())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn http_deflate_is_zlib_wrapped_not_raw() {
        assert_eq!(DecompressionMethod::Deflate.format(), Format::Zlib);
        assert_eq!(Format::Zlib.content_encoding(), Some("deflate"));
    }

    #[test]
    fn every_method_maps_to_a_format_with_an_http_token() {
        for method in DecompressionMethod::ALL {
            assert!(method.format().content_encoding().is_some(), "method {method:?}");
        }
    }

    #[test]
    fn deflate_is_never_preferred() {
        // Servers disagree over whether `deflate` is zlib-wrapped or raw, so it
        // is worth accepting but must always rank below the alternatives.
        assert_eq!(DecompressionMethod::ALL.last().copied(), Some(DecompressionMethod::Deflate));
    }

    #[test]
    fn constructors_and_method_conversions_leave_codec_limits_untouched() {
        for options in [
            ResponseDecompressionOptions::new(),
            ResponseDecompressionOptions::default(),
            ResponseDecompressionOptions::new().methods(DecompressionMethod::ALL),
            ResponseDecompressionOptions::from(DecompressionMethod::ALL),
            ResponseDecompressionOptions::from(&[DecompressionMethod::Gzip]),
        ] {
            assert!(options.limits().is_none());
        }
    }

    #[test]
    fn explicit_unbounded_output_is_distinct_from_inheritance() {
        let options = ResponseDecompressionOptions::new().max_output_len(None);
        assert_eq!(
            options.limits(),
            Some(compressors::DecompressorLimits::new().unbounded_output_len())
        );
    }

    #[test]
    #[should_panic(expected = "max_output_len must be greater than zero")]
    fn zero_output_limit_is_a_configuration_error() {
        let _ = ResponseDecompressionOptions::new().max_output_len(0);
    }

    #[test]
    #[should_panic(expected = "max_output_len must be greater than zero")]
    fn an_optional_zero_output_limit_is_a_configuration_error() {
        let _ = ResponseDecompressionOptions::new().max_output_len(Some(0));
    }

    #[test]
    #[should_panic(expected = "max_streams must be greater than zero")]
    fn zero_stream_limit_is_a_configuration_error() {
        let _ = ResponseDecompressionOptions::new().max_streams(0);
    }
}
