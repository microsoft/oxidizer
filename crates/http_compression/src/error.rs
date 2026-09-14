// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error labels and translation for HTTP body compression and decompression.

use http_extensions::HttpError;
use ohno::ErrorLabel;
use seatbelt::RecoveryInfo;

/// A configured compressible media type is not valid.
#[derive(Debug)]
pub struct CompressibleTypeError {
    value: String,
    source: mime::FromStrError,
}

impl CompressibleTypeError {
    pub(crate) fn new(value: String, source: mime::FromStrError) -> Self {
        Self { value, source }
    }
}

impl std::fmt::Display for CompressibleTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}' is not a valid media type", self.value)
    }
}

impl std::error::Error for CompressibleTypeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The body was malformed for its declared compression format.
pub(crate) const LABEL_COMPRESSION_INVALID: ErrorLabel = ErrorLabel::from_static("compression_invalid");

/// The body declares a compression format that is not enabled.
pub(crate) const LABEL_COMPRESSION_UNSUPPORTED: ErrorLabel = ErrorLabel::from_static("compression_unsupported");

/// Decompressing the body would have exceeded the configured limits.
pub(crate) const LABEL_COMPRESSION_LIMIT_EXCEEDED: ErrorLabel = ErrorLabel::from_static("compression_limit_exceeded");

/// Builds a `compression_unsupported` failure for `token`.
pub(crate) fn unsupported(token: &str) -> HttpError {
    HttpError::other(
        format!("body uses the '{token}' compression format, which is not enabled here"),
        RecoveryInfo::never(),
        LABEL_COMPRESSION_UNSUPPORTED,
    )
}

/// Builds a `compression_invalid` failure from `cause`.
pub(crate) fn invalid(cause: impl std::error::Error + Send + Sync + 'static) -> HttpError {
    HttpError::other(cause, RecoveryInfo::never(), LABEL_COMPRESSION_INVALID)
}

/// Builds a `compression_limit_exceeded` failure for an excessive coding stack.
pub(crate) fn too_many_content_codings(max_layers: usize) -> HttpError {
    HttpError::other(
        format!("body uses more than {max_layers} content-coding layers"),
        RecoveryInfo::never(),
        LABEL_COMPRESSION_LIMIT_EXCEEDED,
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error as _;

    use ohno::Labeled as _;

    use super::*;

    #[test]
    fn invalid_preserves_the_cause_and_label() {
        let error = invalid(std::io::Error::other("bad compressed bytes"));

        assert_eq!(error.label(), "compression_invalid");
        assert!(error.to_string().contains("bad compressed bytes"));
        assert!(error.source().is_some());
    }
}
