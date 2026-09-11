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
