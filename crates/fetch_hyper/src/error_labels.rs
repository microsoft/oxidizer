// Copyright (c) Microsoft Corporation.

//! Centralized [`ErrorLabel`] constants for errors raised by this crate.

#![allow(
    dead_code,
    reason = "labels are exhaustively defined for the surface area even if some are only used under specific features"
)]

use http_extensions::HttpError;
use ohno::{ErrorLabel, Labeled};

pub(crate) const LABEL_CONNECT: ErrorLabel = ErrorLabel::from_static("connect");
pub(crate) const LABEL_REQUEST_HYPER: ErrorLabel = ErrorLabel::from_static("request_hyper");
pub(crate) const LABEL_HTTP_VERSION_UNSUPPORTED: ErrorLabel = ErrorLabel::from_static("http_version_unsupported");
pub(crate) const LABEL_TLS: ErrorLabel = ErrorLabel::from_static("tls");

/// Walks the error chain and concatenates labels into a single [`ErrorLabel`].
pub(crate) fn collect_error_labels(error: &(dyn std::error::Error + 'static)) -> ErrorLabel {
    ErrorLabel::from_error_chain(error, resolve_error_label)
}

fn resolve_error_label(error: &(dyn std::error::Error + 'static)) -> Option<ErrorLabel> {
    if let Some(err) = error.downcast_ref::<HttpError>() {
        return Some(err.label().clone());
    }

    if let Some(err) = error.downcast_ref::<std::io::Error>() {
        return Some(resolve_io_error_label(err));
    }

    None
}

fn resolve_io_error_label(error: &std::io::Error) -> ErrorLabel {
    if let Some(inner) = error.get_ref() {
        if let Some(inner_io) = inner.downcast_ref::<std::io::Error>() {
            return resolve_io_error_label(inner_io);
        }

        #[cfg(feature = "rustls")]
        if inner.downcast_ref::<rustls::Error>().is_some() {
            return LABEL_TLS;
        }

        #[cfg(feature = "native-tls")]
        if inner.downcast_ref::<native_tls::Error>().is_some() {
            return LABEL_TLS;
        }
    }

    error.kind().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_returns_label_for_http_error() {
        let err = HttpError::other("boom", seatbelt::RecoveryInfo::never(), LABEL_CONNECT);
        let label = collect_error_labels(&err);
        // HttpError carries a label; collect should produce something containing it.
        assert!(label.as_str().contains("connect"));
    }

    #[test]
    fn collect_handles_chained_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let label = collect_error_labels(&io);
        assert!(!label.as_str().is_empty());
    }
}
