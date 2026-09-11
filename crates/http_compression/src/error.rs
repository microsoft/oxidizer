// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error labels and translation for HTTP body compression and decompression.

use std::fmt::Display;

use http_extensions::HttpError;
use ohno::ErrorLabel;
use seatbelt::RecoveryInfo;

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
pub(crate) fn invalid(cause: impl Display) -> HttpError {
    HttpError::other(cause.to_string(), RecoveryInfo::never(), LABEL_COMPRESSION_INVALID)
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
    use ohno::Labeled as _;

    use super::*;

    #[test]
    fn invalid_preserves_the_cause_text_and_label() {
        let error = invalid("bad compressed bytes");

        assert_eq!(error.label(), "compression_invalid");
        assert!(error.to_string().contains("bad compressed bytes"));
    }
}
