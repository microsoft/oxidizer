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
    let Some(inner) = error.get_ref() else {
        return error.kind().into();
    };

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

    error.kind().into()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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

    #[test]
    fn nested_io_error_unwraps_inner_kind() {
        // Outer io::Error wraps an inner io::Error with a distinguishable kind.
        let inner = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let outer = std::io::Error::other(inner);
        let label = resolve_io_error_label(&outer);
        // inner kind should win over outer "other" kind
        let kind_label: ohno::ErrorLabel = std::io::ErrorKind::ConnectionRefused.into();
        assert_eq!(label.as_str(), kind_label.as_str());
    }

    #[test]
    fn io_error_without_inner_uses_kind() {
        let io = std::io::Error::from(std::io::ErrorKind::ConnectionAborted);
        let label = resolve_io_error_label(&io);
        let kind_label: ohno::ErrorLabel = std::io::ErrorKind::ConnectionAborted.into();
        assert_eq!(label.as_str(), kind_label.as_str());
    }

    #[cfg(feature = "native-tls")]
    #[test]
    fn native_tls_error_in_io_chain_resolves_to_tls_label() {
        // Force a native_tls::Error by attempting to build an identity from invalid PKCS#12 data.
        let Err(native_err) = native_tls::Identity::from_pkcs12(b"not-a-real-pkcs12", "wrong") else {
            panic!("invalid pkcs12 must fail");
        };
        let io = std::io::Error::other(native_err);
        let label = resolve_io_error_label(&io);
        assert_eq!(label.as_str(), LABEL_TLS.as_str());
    }

    #[cfg(feature = "rustls")]
    #[test]
    fn rustls_error_in_io_chain_resolves_to_tls_label() {
        let rustls_err = rustls::Error::General("synthetic".to_string());
        let io = std::io::Error::other(rustls_err);
        let label = resolve_io_error_label(&io);
        assert_eq!(label.as_str(), LABEL_TLS.as_str());
    }

    #[test]
    fn resolve_returns_none_for_unrecognized_error() {
        #[derive(Debug)]
        struct UnknownErr;
        impl std::fmt::Display for UnknownErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("unknown")
            }
        }
        impl std::error::Error for UnknownErr {}
        assert!(resolve_error_label(&UnknownErr).is_none());
    }

    #[test]
    fn resolve_returns_io_label_for_io_error() {
        let io = std::io::Error::from(std::io::ErrorKind::TimedOut);
        let label = resolve_error_label(&io).expect("io error should yield a label");
        let expected: ohno::ErrorLabel = std::io::ErrorKind::TimedOut.into();
        assert_eq!(label.as_str(), expected.as_str());
    }
}
