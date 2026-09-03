// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use recoverable::{Recovery, RecoveryInfo};

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

impl Kind {
    /// How a failure of this kind is recovered from, absent anything more specific.
    fn default_recovery(self) -> RecoveryInfo {
        match self {
            // Truncation is worth another attempt only if the bytes are fetched again, and this
            // crate neither owns nor re-drives the source: the operation that raised this is a pure
            // decode of bytes already in hand, so re-running it is guaranteed to fail identically.
            // Whoever owns the transport is the layer that can classify this, exactly as for a
            // foreign failure below.
            Self::UnexpectedEndOfStream | Self::Source => RecoveryInfo::unknown(),
            // Malformed input stays malformed, a bound stays exceeded, and misuse or a rejected
            // setting needs a code change rather than another attempt.
            Self::CorruptData | Self::LimitExceeded | Self::InvalidState | Self::InvalidConfiguration => RecoveryInfo::never(),
        }
    }
}

/// Classifies a foreign error by looking for an [`io::Error`][std::io::Error] in its chain.
///
/// An engine or transport failure is usually an IO failure wearing a wrapper, and `recoverable`
/// already classifies every [`ErrorKind`][std::io::ErrorKind]. Anything else is unknown rather than
/// guessed at.
fn detect_recovery(source: &(dyn StdError + 'static)) -> RecoveryInfo {
    let mut current = Some(source);

    while let Some(error) = current {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return RecoveryInfo::from(io.kind());
        }

        current = error.source();
    }

    RecoveryInfo::unknown()
}

/// An error produced while compressing or decompressing.
///
/// This is a single canonical error type rather than an enum, so that new failure modes do not
/// break downstream `match` statements. Classify a failure with the `is_*` accessors, or with
/// [`recovery`][Recovery::recovery] when what matters is whether retrying could help.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use compressors::{Resources, gzip};
///
/// let error = gzip::decompress(b"definitely not gzip", &Resources::default()).unwrap_err();
/// assert!(error.is_corrupt_data());
/// # }
/// ```
#[derive(Debug)]
pub struct Error {
    kind: Kind,
    message: Cow<'static, str>,
    source: Option<Box<dyn StdError + Send + Sync>>,
    recovery: RecoveryInfo,
}

#[cfg_attr(
    all(
        not(test),
        not(any(
            test,
            feature = "brotli",
            feature = "deflate",
            feature = "gzip",
            feature = "zlib",
            feature = "zstd"
        ))
    ),
    expect(dead_code, reason = "only the codecs construct these, and no format is enabled")
)]
impl Error {
    pub(crate) fn new(kind: Kind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
            recovery: kind.default_recovery(),
        }
    }

    #[cfg_attr(
        all(not(test), not(any(feature = "deflate", feature = "gzip", feature = "zlib"))),
        expect(dead_code, reason = "only the flate codecs attach a source")
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
}

impl Error {
    /// Wraps a foreign error, classifying it by inspecting it.
    ///
    /// Use this to carry a failure from something this crate drives -- a transport, a reader, an
    /// engine binding -- through an API that returns this crate's [`Error`]. The wrapped error stays
    /// reachable through [`source`][std::error::Error::source], and
    /// [`is_source`][Self::is_source] reports the resulting error.
    ///
    /// The recovery information is detected rather than assumed: if an
    /// [`io::Error`][std::io::Error] appears anywhere in `source`'s chain, its
    /// [`ErrorKind`][std::io::ErrorKind] decides the classification. Anything else is
    /// [`RecoveryInfo::unknown`]. Reach for [`other_with_recovery`][Self::other_with_recovery] when
    /// you already know better than the heuristic.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    ///
    /// use compressors::Error;
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// let error = Error::other(
    ///     "reading the body failed",
    ///     io::Error::from(io::ErrorKind::TimedOut),
    /// );
    ///
    /// assert!(error.is_source());
    /// assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    /// ```
    #[must_use]
    pub fn other(message: impl Into<Cow<'static, str>>, source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        let source = source.into();
        let recovery = detect_recovery(&*source);

        Self::other_with_recovery(message, source, recovery)
    }

    /// Wraps a foreign error with recovery information you supply.
    ///
    /// Identical to [`other`][Self::other] except that `recovery` is attached as given, for when the
    /// caller knows how the failure should be handled and the heuristic cannot.
    ///
    /// # Examples
    ///
    /// ```
    /// use compressors::Error;
    /// use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
    ///
    /// #[derive(Debug)]
    /// struct Throttled;
    /// # impl std::fmt::Display for Throttled {
    /// #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    /// #         f.write_str("throttled")
    /// #     }
    /// # }
    /// # impl std::error::Error for Throttled {}
    ///
    /// let error = Error::other_with_recovery(
    ///     "the backend throttled us",
    ///     Throttled,
    ///     RecoveryInfo::unavailable(),
    /// );
    ///
    /// assert_eq!(error.recovery().kind(), RecoveryKind::Unavailable);
    /// ```
    #[must_use]
    pub fn other_with_recovery(
        message: impl Into<Cow<'static, str>>,
        source: impl Into<Box<dyn StdError + Send + Sync>>,
        recovery: RecoveryInfo,
    ) -> Self {
        let mut error = Self::new(Kind::Source, message);
        error.source = Some(source.into());
        error.recovery = recovery;
        error
    }

    /// The compressed data is malformed, or its checksum does not match the decompressed bytes.
    #[must_use]
    pub fn is_corrupt_data(&self) -> bool {
        self.kind == Kind::CorruptData
    }

    /// The input ended in the middle of a compressed stream.
    ///
    /// The bytes decompressed so far are valid; the producer stopped early or the transport truncated
    /// them. This is distinct from [`is_corrupt_data`][Self::is_corrupt_data] because fetching the
    /// body again may well produce a complete one, whereas corrupt data stays corrupt.
    ///
    /// That is advice for whoever owns the byte source, not for a retry of this call: decompressing
    /// the same buffer again is deterministic and fails the same way, which is why
    /// [`recovery`][recoverable::Recovery::recovery] reports this as
    /// [`Unknown`][recoverable::RecoveryKind::Unknown] rather than asserting a retry to middleware
    /// that cannot re-drive the transport.
    #[must_use]
    pub fn is_unexpected_end_of_stream(&self) -> bool {
        self.kind == Kind::UnexpectedEndOfStream
    }

    /// Decompression would have exceeded the configured [`DecompressorLimits`].
    ///
    /// [`DecompressorLimits`]: crate::DecompressorLimits
    #[must_use]
    pub fn is_limit_exceeded(&self) -> bool {
        self.kind == Kind::LimitExceeded
    }

    /// The engine was driven in an order it does not support, such as pushing input after end of
    /// input, or the underlying compression engine reported an internal failure.
    #[must_use]
    pub fn is_invalid_state(&self) -> bool {
        self.kind == Kind::InvalidState
    }

    /// A configuration value was outside the range the format accepts.
    ///
    /// Produced by the `TryFrom` conversions on types such as `Level`, where the
    /// value typically came from a configuration file or a command line.
    #[must_use]
    pub fn is_invalid_configuration(&self) -> bool {
        self.kind == Kind::InvalidConfiguration
    }

    /// The stream feeding the engine failed.
    ///
    /// The compressed data itself was fine as far as it went; the source could not deliver more.
    /// The original failure is available from [`source`][std::error::Error::source]. Produced by
    /// [`other`][Self::other] and [`other_with_recovery`][Self::other_with_recovery], and by the
    /// adapters behind the `futures-stream` feature.
    #[must_use]
    pub fn is_source(&self) -> bool {
        self.kind == Kind::Source
    }
}

impl Recovery for Error {
    /// Whether retrying could help, and how soon.
    ///
    /// Kinds this crate raises itself are classified by what they mean: a truncated stream is worth
    /// another attempt, while corrupt data, an exceeded bound, misuse and a rejected setting are
    /// not. A wrapped foreign error reports whatever [`other`][Self::other] detected or
    /// [`other_with_recovery`][Self::other_with_recovery] was given.
    fn recovery(&self) -> RecoveryInfo {
        self.recovery.clone()
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
/// return this at all. The exception is zstd, whose native library validates the parameters this
/// crate hands it and can therefore reject them.
///
/// This is a separate type from [`Error`] so that a failure to build is not something callers have
/// to consider while streaming: once an engine exists, this error can no longer occur. It converts
/// into [`Error`] for code that handles both in one place.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "zstd")]
/// # {
/// use compressors::{Resources, zstd};
///
/// let compressor = zstd::Compressor::builder().build(&Resources::default())?;
/// # let _ = compressor;
/// # }
/// # Ok::<(), compressors::BuildError>(())
/// ```
#[derive(Debug)]
pub struct BuildError {
    message: Cow<'static, str>,
}

#[cfg_attr(
    all(not(test), not(feature = "zstd")),
    expect(dead_code, reason = "only the zstd engine validates a configuration")
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

impl Recovery for BuildError {
    /// A rejected setting needs a code change rather than another attempt.
    fn recovery(&self) -> RecoveryInfo {
        RecoveryInfo::never()
    }
}

impl From<BuildError> for Error {
    fn from(error: BuildError) -> Self {
        Self::new(Kind::InvalidConfiguration, error.message)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use recoverable::RecoveryKind;

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
    fn is_source_reports_only_the_source_kind() {
        let error = Error::other("the underlying stream failed", std::io::Error::other("stream failed"));

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

    #[test]
    fn each_kind_classifies_its_own_recoverability() {
        let cases = [
            (Error::unexpected_end_of_stream(), RecoveryKind::Unknown),
            (Error::corrupt_data("bad"), RecoveryKind::Never),
            (Error::output_limit_exceeded(2, 1), RecoveryKind::Never),
            (Error::invalid_state("wrong order"), RecoveryKind::Never),
            (Error::invalid_configuration("out of range"), RecoveryKind::Never),
        ];

        for (error, expected) in cases {
            assert_eq!(error.recovery().kind(), expected, "wrong recovery for {error}");
        }
    }

    #[test]
    fn a_build_failure_converts_to_an_unrecoverable_error() {
        let build_error = BuildError::new("the engine rejected the window size");

        assert_eq!(
            build_error.recovery().kind(),
            RecoveryKind::Never,
            "a rejected setting needs a code change"
        );

        let error = Error::from(build_error);

        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
    }

    #[test]
    fn other_classifies_a_wrapped_io_error_by_its_kind() {
        let cases = [
            (std::io::ErrorKind::TimedOut, RecoveryKind::Retry),
            (std::io::ErrorKind::NetworkDown, RecoveryKind::Unavailable),
            (std::io::ErrorKind::NotFound, RecoveryKind::Never),
        ];

        for (kind, expected) in cases {
            let error = Error::other("the transport failed", std::io::Error::from(kind));

            assert!(error.is_source(), "got {error}");
            assert_eq!(error.to_string(), "the transport failed");
            assert_eq!(error.recovery().kind(), expected, "wrong recovery for {kind:?}");
        }
    }

    #[test]
    fn other_finds_an_io_error_nested_inside_a_wrapper() {
        // A transport rarely hands back a bare io::Error; the heuristic has to look through
        // whatever wrapped it.
        #[derive(Debug)]
        struct Wrapper(std::io::Error);

        impl fmt::Display for Wrapper {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("wrapped")
            }
        }

        impl StdError for Wrapper {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&self.0)
            }
        }

        let error = Error::other(
            "the transport failed",
            Wrapper(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
        );

        assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
        assert_eq!(
            error.source().expect("the wrapper was attached").to_string(),
            "wrapped",
            "the wrapper itself stays the reported cause, not the io::Error the heuristic reached through it"
        );
    }

    #[test]
    fn other_reports_unknown_when_nothing_in_the_chain_is_an_io_error() {
        let error = Error::other("something else failed", "a plain message");

        assert_eq!(error.recovery().kind(), RecoveryKind::Unknown);
        assert_eq!(error.source().expect("the cause was attached").to_string(), "a plain message");
    }

    #[test]
    fn other_with_recovery_attaches_what_it_was_given() {
        let supplied = RecoveryInfo::unavailable();
        let error = Error::other_with_recovery("the backend is degraded", "a plain message", supplied.clone());

        assert!(error.is_source(), "got {error}");
        assert_eq!(error.recovery(), supplied, "the supplied information should win over the heuristic");
    }

    #[test]
    fn other_with_recovery_overrides_what_the_heuristic_would_have_detected() {
        // The caller knows the timeout is terminal for them even though the heuristic says retry.
        let error = Error::other_with_recovery(
            "the deadline is gone",
            std::io::Error::from(std::io::ErrorKind::TimedOut),
            RecoveryInfo::never(),
        );

        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
    }

    #[test]
    fn a_wrapped_cause_carries_its_own_recovery() {
        let error = Error::other(
            "the underlying stream failed",
            std::io::Error::from(std::io::ErrorKind::ConnectionReset),
        );

        assert!(error.is_source(), "got {error}");
        assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    }
}
