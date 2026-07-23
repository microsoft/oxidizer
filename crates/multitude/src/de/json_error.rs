// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::error::Error;
use core::fmt;

use super::{DeserializationResource, LimitExceeded};

#[derive(Debug)]
enum JsonErrorKind {
    Json,
    LimitExceeded(LimitExceeded),
}

/// An error from resource-limited arena-aware JSON deserialization.
///
/// Use [`limit_exceeded`](Self::limit_exceeded) to distinguish resource
/// rejection from malformed or incompatible JSON. The underlying
/// [`serde_json::Error`] remains available through
/// [`as_json_error`](Self::as_json_error) and [`Error::source`].
/// [`Display`](fmt::Display) reports only the high-level classification so
/// error reporters can render the source chain and backtrace without
/// duplicating them.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::{DeserializationLimits, DeserializationResource};
///
/// let arena = Arena::new();
/// let limits = DeserializationLimits::unlimited().with_max_sequence_len(1);
/// let error = arena
///     .deserialize_json_with_limits::<multitude::Box<[u64]>, _>("[1,2]", limits)
///     .unwrap_err();
/// let exceeded = error.limit_exceeded().unwrap();
/// assert_eq!(exceeded.resource(), DeserializationResource::SequenceLength);
/// assert_eq!(exceeded.limit(), 1);
/// ```
#[derive(Debug)]
pub struct JsonError {
    kind: JsonErrorKind,
    source: serde_json::Error,
    #[cfg(feature = "std")]
    backtrace: std::backtrace::Backtrace,
}

impl JsonError {
    #[cold]
    pub(super) fn new(source: serde_json::Error, limit_exceeded: Option<LimitExceeded>) -> Self {
        Self {
            kind: limit_exceeded.map_or(JsonErrorKind::Json, JsonErrorKind::LimitExceeded),
            source,
            #[cfg(feature = "std")]
            backtrace: std::backtrace::Backtrace::capture(),
        }
    }

    /// Returns details when a configured resource limit was exceeded.
    #[must_use]
    pub const fn limit_exceeded(&self) -> Option<LimitExceeded> {
        match self.kind {
            JsonErrorKind::Json => None,
            JsonErrorKind::LimitExceeded(details) => Some(details),
        }
    }

    /// Returns whether a configured resource limit was exceeded.
    #[must_use]
    pub const fn is_limit_exceeded(&self) -> bool {
        self.limit_exceeded().is_some()
    }

    /// Returns the underlying JSON error.
    ///
    /// Parse and shape errors retain their input location. Errors synthesized
    /// from failures outside the parser, such as root allocation, have no
    /// meaningful line or column.
    #[must_use]
    pub const fn as_json_error(&self) -> &serde_json::Error {
        &self.source
    }

    /// Returns the backtrace captured when this error was constructed.
    #[cfg(feature = "std")]
    pub const fn backtrace(&self) -> &std::backtrace::Backtrace {
        &self.backtrace
    }
}

impl From<serde_json::Error> for JsonError {
    fn from(source: serde_json::Error) -> Self {
        Self::new(source, None)
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            JsonErrorKind::Json => f.write_str("JSON deserialization failed"),
            JsonErrorKind::LimitExceeded(details) => {
                let resource = match details.resource() {
                    DeserializationResource::Depth => "nesting depth",
                    DeserializationResource::SequenceLength => "sequence length",
                    DeserializationResource::MapLength => "map length",
                    DeserializationResource::StringLength => "string length",
                    DeserializationResource::ByteStringLength => "byte string length",
                };
                f.write_str("JSON deserialization exceeded the ")?;
                f.write_str(resource)?;
                write!(f, " limit of {}", details.limit())
            }
        }
    }
}

impl Error for JsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}
