// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`TranscodeError`] request/response transcoding error type.

use core::fmt;
use std::backtrace::Backtrace;
use std::error::Error;

use crate::handling::{Code, Status};

/// An error produced while transcoding a request or response.
///
/// The underlying `serde`/`serde_json` failure (when there is one) is preserved
/// and reachable through [`std::error::Error::source`]. A [`Backtrace`] is
/// captured at construction and shown in the `Debug` output when
/// `RUST_BACKTRACE` is enabled.
///
/// # Examples
///
/// ```
/// use std::error::Error as _;
///
/// use rest_over_grpc::codegen_helpers::{RequestBodyKind, decode_request};
/// use rest_over_grpc::handling::Code;
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize)]
/// struct Shelf {
///     shelf: String,
/// }
///
/// let error = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Whole)
///     .expect_err("body is not valid JSON");
///
/// assert_eq!(error.code(), Code::InvalidArgument);
/// assert!(error.source().is_some());
/// ```
#[derive(Debug)]
pub struct TranscodeError {
    kind: TranscodeErrorKind,
    detail: String,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
    #[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")]
    backtrace: Box<Backtrace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscodeErrorKind {
    Body,
    Structure,
    Deserialize,
    Serialize,
}

impl TranscodeError {
    #[expect(
        private_interfaces,
        reason = "TranscodeErrorKind intentionally stays private; sibling modules use the typed constructors"
    )]
    pub(crate) fn from_source(kind: TranscodeErrorKind, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            kind,
            detail: source.to_string(),
            source: Some(Box::new(source)),
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    pub(crate) fn body(source: serde_json::Error) -> Self {
        Self::from_source(TranscodeErrorKind::Body, source)
    }

    pub(crate) fn deserialize(source: serde_json::Error) -> Self {
        Self::from_source(TranscodeErrorKind::Deserialize, source)
    }

    pub(crate) fn body_or_deserialize(source: serde_json::Error) -> Self {
        if source.is_syntax() || source.is_eof() {
            Self::body(source)
        } else {
            Self::deserialize(source)
        }
    }

    pub(crate) fn deserialize_value(source: serde::de::value::Error) -> Self {
        Self::from_source(TranscodeErrorKind::Deserialize, source)
    }

    pub(crate) fn serialize(source: serde_json::Error) -> Self {
        Self::from_source(TranscodeErrorKind::Serialize, source)
    }

    pub(crate) fn serialize_message(detail: String) -> Self {
        Self {
            kind: TranscodeErrorKind::Serialize,
            detail,
            source: None,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    pub(crate) fn structure(detail: &str) -> Self {
        Self {
            kind: TranscodeErrorKind::Structure,
            detail: detail.to_owned(),
            source: None,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    pub(crate) fn response_structure(detail: &str) -> Self {
        Self {
            kind: TranscodeErrorKind::Serialize,
            detail: detail.to_owned(),
            source: None,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    pub(crate) fn invalid_encoding(component: &str) -> Self {
        Self {
            kind: TranscodeErrorKind::Deserialize,
            detail: format!("{component} contains malformed percent encoding or invalid UTF-8"),
            source: None,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    /// Reports a path value that did not parse into its destination field.
    pub(crate) fn path_field(value: &str, error: &dyn fmt::Display) -> Self {
        Self {
            kind: TranscodeErrorKind::Deserialize,
            detail: format!("path variable value {value:?} did not parse: {error}"),
            source: None,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    /// Reports an unknown numeric or named enum value.
    pub(crate) fn path_enum(value: &str) -> Self {
        Self {
            kind: TranscodeErrorKind::Deserialize,
            detail: format!("path variable value {value:?} is not a valid enum value"),
            source: None,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    /// The gRPC [`Code`] this error maps to: request-side failures are
    /// [`Code::InvalidArgument`]; response-side failures are [`Code::Internal`].
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::codegen_helpers::{RequestBodyKind, decode_request};
    /// use rest_over_grpc::handling::Code;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Shelf {
    ///     shelf: String,
    /// }
    ///
    /// let error = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Whole)
    ///     .expect_err("body is not valid JSON");
    /// assert_eq!(error.code(), Code::InvalidArgument);
    /// ```
    #[must_use]
    pub const fn code(&self) -> Code {
        match self.kind {
            TranscodeErrorKind::Body | TranscodeErrorKind::Structure | TranscodeErrorKind::Deserialize => Code::InvalidArgument,
            TranscodeErrorKind::Serialize => Code::Internal,
        }
    }

    /// Converts this error into a [`Status`] for reporting to the client.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::codegen_helpers::{RequestBodyKind, decode_request};
    /// use rest_over_grpc::handling::Code;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Shelf {
    ///     shelf: String,
    /// }
    ///
    /// let error = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Whole)
    ///     .expect_err("body is not valid JSON");
    /// let status = error.into_status();
    /// assert_eq!(status.code(), Code::InvalidArgument);
    /// assert!(status.message().contains("invalid request body JSON"));
    /// ```
    #[must_use]
    pub fn into_status(self) -> Status {
        Status::new(self.code(), format!("{}: {}", self.summary(), self.detail))
    }

    /// The human-readable summary for this error's kind.
    const fn summary(&self) -> &'static str {
        match self.kind {
            TranscodeErrorKind::Body => "invalid request body JSON",
            TranscodeErrorKind::Structure => "invalid request structure",
            TranscodeErrorKind::Deserialize => "request did not match the expected message",
            TranscodeErrorKind::Serialize => "failed to encode the response",
        }
    }
}

impl From<TranscodeError> for Status {
    fn from(error: TranscodeError) -> Self {
        error.into_status()
    }
}

impl fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.summary(), self.detail)
    }
}

impl Error for TranscodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(|source| source.as_ref() as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_or_deserialize_classifies_json_errors() {
        let syntax = serde_json::from_slice::<u32>(b"not json").expect_err("invalid JSON");
        assert_eq!(TranscodeError::body_or_deserialize(syntax).kind, TranscodeErrorKind::Body);

        let shape = serde_json::from_slice::<u32>(br#""text""#).expect_err("wrong JSON shape");
        assert_eq!(TranscodeError::body_or_deserialize(shape).kind, TranscodeErrorKind::Deserialize);
    }
}
