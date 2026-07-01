// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`TranscodeError`] request/response transcoding error type.

use core::fmt;
use std::backtrace::Backtrace;

use crate::{Code, Status};

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
/// use rest_over_grpc::transcode::{BodyKind, decode_request};
/// use rest_over_grpc::{Binding, Code};
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize)]
/// struct Shelf {
///     shelf: String,
/// }
///
/// let error = decode_request::<Shelf>(
///     &[Binding::new(&["shelf"], "7")],
///     &[],
///     b"not json",
///     BodyKind::Whole,
/// )
/// .expect_err("body is not valid JSON");
///
/// assert_eq!(error.code(), Code::InvalidArgument);
/// assert!(error.source().is_some());
/// ```
#[derive(Debug)]
pub struct TranscodeError {
    kind: TranscodeErrorKind,
    detail: String,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
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
    pub(crate) fn from_source(kind: TranscodeErrorKind, source: impl std::error::Error + Send + Sync + 'static) -> Self {
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

    pub(crate) fn deserialize_value(source: serde::de::value::Error) -> Self {
        Self::from_source(TranscodeErrorKind::Deserialize, source)
    }

    pub(crate) fn serialize(source: serde_json::Error) -> Self {
        Self::from_source(TranscodeErrorKind::Serialize, source)
    }

    pub(crate) fn structure(detail: &str) -> Self {
        Self {
            kind: TranscodeErrorKind::Structure,
            detail: detail.to_owned(),
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
    /// use rest_over_grpc::transcode::{BodyKind, decode_request};
    /// use rest_over_grpc::{Binding, Code};
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Shelf {
    ///     shelf: String,
    /// }
    ///
    /// let error = decode_request::<Shelf>(
    ///     &[Binding::new(&["shelf"], "7")],
    ///     &[],
    ///     b"not json",
    ///     BodyKind::Whole,
    /// )
    /// .expect_err("body is not valid JSON");
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
    /// use rest_over_grpc::transcode::{BodyKind, decode_request};
    /// use rest_over_grpc::{Binding, Code};
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Shelf {
    ///     shelf: String,
    /// }
    ///
    /// let error = decode_request::<Shelf>(
    ///     &[Binding::new(&["shelf"], "7")],
    ///     &[],
    ///     b"not json",
    ///     BodyKind::Whole,
    /// )
    /// .expect_err("body is not valid JSON");
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
            TranscodeErrorKind::Serialize => "failed to serialize the response message",
        }
    }
}

impl From<TranscodeError> for Status {
    /// Renders the error as a client-facing [`Status`] (see
    /// [`TranscodeError::into_status`]).
    fn from(error: TranscodeError) -> Self {
        error.into_status()
    }
}

impl fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.summary(), self.detail)
    }
}

impl std::error::Error for TranscodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}
