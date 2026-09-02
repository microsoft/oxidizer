// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

/// The failure mode of an [`Error`].
///
/// Deliberately private: keeping the discriminants out of the public API means new failure modes
/// can be added without a breaking change. Consumers branch on the `is_*` accessors instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    CorruptData,
    UnexpectedEndOfStream,
    LimitExceeded,
    InvalidState,
    InvalidConfiguration,
    Source,
}

/// An error produced while compressing or decompressing.
///
/// This is a single canonical error type rather than an enum, so that new failure modes do not
/// break downstream `match` statements. Classify a failure with the `is_*` accessors.
///
/// # Examples
///
/// ```
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::GlobalPool;
/// use compressors::{Resources, gzip};
///
/// let memory = GlobalPool::new();
/// let not_gzip = BytesView::copied_from_slice(b"definitely not gzip", &memory);
///
/// let error = gzip::decompress(not_gzip, &Resources::default()).unwrap_err();
/// assert!(error.is_corrupt_data());
/// ```
#[derive(Debug)]
pub struct Error {
    kind: Kind,
    message: Cow<'static, str>,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

#[cfg_attr(
    all(
        not(test),
        not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))
    ),
    expect(dead_code, reason = "only the codecs construct these, and no format is enabled")
)]
impl Error {
    pub(crate) fn new(kind: Kind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    #[cfg_attr(
        all(
            not(test),
            not(any(feature = "deflate", feature = "futures-stream", feature = "gzip", feature = "zlib"))
        ),
        expect(dead_code, reason = "only the flate codecs and the stream adapters attach a source")
    )]
    pub(crate) fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub(crate) fn corrupt_data(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Kind::CorruptData, message)
    }

    pub(crate) fn unexpected_end_of_stream() -> Self {
        Self::new(Kind::UnexpectedEndOfStream, "compressed stream ended before the final block")
    }

    pub(crate) fn output_limit_exceeded(actual: u64, maximum: u64) -> Self {
        Self::new(
            Kind::LimitExceeded,
            format!("decompressed output reached {actual} bytes, exceeding the limit of {maximum}"),
        )
    }

    pub(crate) fn ratio_limit_exceeded(input: u64, output: u64, maximum: u32) -> Self {
        Self::new(
            Kind::LimitExceeded,
            format!(
                "decompressed output reached {output} bytes from {input} compressed bytes, \
                 exceeding the expansion limit of {maximum}x"
            ),
        )
    }

    pub(crate) fn stream_limit_exceeded(actual: u64, maximum: u64) -> Self {
        Self::new(
            Kind::LimitExceeded,
            format!("decoded stream count reached {actual}, exceeding the limit of {maximum}"),
        )
    }

    pub(crate) fn invalid_state(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Kind::InvalidState, message)
    }

    pub(crate) fn invalid_configuration(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Kind::InvalidConfiguration, message)
    }

    #[cfg(feature = "futures-stream")]
    pub(crate) fn source(source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        let mut error = Self::new(Kind::Source, "the underlying stream failed");
        error.source = Some(source.into());
        error
    }
}

impl Error {
    /// The compressed data is malformed, or its checksum does not match the decompressed bytes.
    #[must_use]
    pub fn is_corrupt_data(&self) -> bool {
        self.kind == Kind::CorruptData
    }

    /// The input ended in the middle of a compressed stream.
    ///
    /// The bytes decompressed so far are valid; the producer stopped early or the transport truncated
    /// them. This is distinct from [`is_corrupt_data`][Self::is_corrupt_data] because it is usually
    /// worth retrying, whereas corrupt data is not.
    #[must_use]
    pub fn is_unexpected_end_of_stream(&self) -> bool {
        self.kind == Kind::UnexpectedEndOfStream
    }

    /// Decompression would have exceeded the configured [`DecompressionLimits`].
    ///
    /// [`DecompressionLimits`]: crate::DecompressionLimits
    #[must_use]
    pub fn is_limit_exceeded(&self) -> bool {
        self.kind == Kind::LimitExceeded
    }

    /// The codec was driven in an order it does not support, such as pushing input after end of
    /// input, or the underlying compression engine reported an internal failure.
    #[must_use]
    pub fn is_invalid_state(&self) -> bool {
        self.kind == Kind::InvalidState
    }

    /// A configuration value was outside the range the format accepts.
    ///
    /// Produced by the `TryFrom` conversions on types such as [`Level`][crate::Level], where the
    /// value typically came from a configuration file or a command line.
    #[must_use]
    pub fn is_invalid_configuration(&self) -> bool {
        self.kind == Kind::InvalidConfiguration
    }

    /// The stream feeding the codec failed.
    ///
    /// The compressed data itself was fine as far as it went; the source could not deliver more.
    /// The original failure is available from [`source`][std::error::Error::source]. Only produced
    /// by the adapters behind the `futures-stream` feature.
    #[must_use]
    pub fn is_source(&self) -> bool {
        self.kind == Kind::Source
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_ref().map(|source| &**source as &(dyn StdError + 'static))
    }
}

/// A [`Result`][std::result::Result] whose error is this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// A compressor or decompressor could not be built from the settings it was given.
///
/// Most formats accept any combination the builders can express, so their `build` methods do not
/// return this at all. The exceptions are the formats whose engines validate their own parameters
/// -- brotli and zstd -- where building applies the configuration and can therefore be rejected.
///
/// This is a separate type from [`Error`] so that a failure to build is not something callers have
/// to consider while streaming: once a codec exists, this error can no longer occur. It converts
/// into [`Error`] for code that handles both in one place.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "brotli")]
/// # {
/// use bytesbuf::mem::GlobalPool;
/// use compressors::{Resources, brotli};
///
/// let compressor = brotli::Compressor::builder().build(&Resources::default())?;
/// # let _ = compressor;
/// # }
/// # Ok::<(), compressors::BuildError>(())
/// ```
#[derive(Debug)]
pub struct BuildError {
    message: Cow<'static, str>,
}

#[cfg_attr(
    all(not(test), not(any(feature = "brotli", feature = "zstd"))),
    expect(dead_code, reason = "only the brotli and zstd engines validate a configuration")
)]
impl BuildError {
    pub(crate) fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for BuildError {}

impl From<BuildError> for Error {
    fn from(error: BuildError) -> Self {
        Self::new(Kind::InvalidConfiguration, error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_failure_renders_and_converts_to_an_invalid_configuration() {
        let error = BuildError::new("the engine rejected the window size");

        assert_eq!(error.to_string(), "the engine rejected the window size");
        assert!(error.source().is_none(), "a build failure has no cause to report");
        assert!(format!("{error:?}").contains("BuildError"), "the kind should be visible");

        let converted = Error::from(error);
        assert!(converted.is_invalid_configuration(), "got {converted}");
        assert_eq!(converted.to_string(), "the engine rejected the window size");
    }

    #[test]
    fn accessors_report_exactly_one_kind() {
        let cases = [
            (Error::corrupt_data("bad"), [true, false, false, false, false, false]),
            (Error::unexpected_end_of_stream(), [false, true, false, false, false, false]),
            (Error::output_limit_exceeded(2, 1), [false, false, true, false, false, false]),
            (Error::invalid_state("wrong order"), [false, false, false, true, false, false]),
            (
                Error::invalid_configuration("out of range"),
                [false, false, false, false, true, false],
            ),
        ];

        for (error, expected) in cases {
            let actual = [
                error.is_corrupt_data(),
                error.is_unexpected_end_of_stream(),
                error.is_limit_exceeded(),
                error.is_invalid_state(),
                error.is_invalid_configuration(),
                error.is_source(),
            ];
            assert_eq!(actual, expected, "wrong classification for {error}");
        }
    }

    #[test]
    #[cfg(feature = "futures-stream")]
    fn is_source_reports_only_the_source_kind() {
        let error = Error::source(std::io::Error::other("stream failed"));

        assert!(error.is_source(), "got {error}");
        assert!(!error.is_corrupt_data(), "got {error}");
        assert!(!error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn display_messages_start_lowercase() {
        let errors = [
            Error::corrupt_data("bad gzip header"),
            Error::unexpected_end_of_stream(),
            Error::output_limit_exceeded(2, 1),
            Error::invalid_state("input already ended"),
        ];

        for error in errors {
            let rendered = error.to_string();
            let first = rendered.chars().next().expect("error messages are never empty");
            assert!(!first.is_uppercase(), "message should not start with a capital: {rendered}");
            assert!(!rendered.contains("exception"), "say 'error', not 'exception': {rendered}");
        }
    }

    #[test]
    fn source_is_exposed_when_present() {
        let inner = std::io::Error::other("inner failure");
        let error = Error::corrupt_data("outer").with_source(inner);

        let source = error.source().expect("source was attached");
        assert_eq!(source.to_string(), "inner failure");
    }

    #[test]
    fn source_is_absent_by_default() {
        assert!(Error::corrupt_data("no cause").source().is_none());
    }

    #[test]
    fn debug_is_available_for_diagnostics() {
        let rendered = format!("{:?}", Error::output_limit_exceeded(2, 1));
        assert!(rendered.contains("LimitExceeded"), "kind should be visible: {rendered}");
    }
}
