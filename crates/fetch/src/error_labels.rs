// Copyright (c) Microsoft Corporation.

#![allow(dead_code, reason = "this code is cheap and adding build conditions just complicates it")]

//! Centralized [`ErrorLabel`] constants for all errors in this crate.

use http_extensions::HttpError;
use ohno::{ErrorLabel, Labeled};

// Connection errors
pub(crate) const LABEL_CONNECT: ErrorLabel = ErrorLabel::from_static("connect");

// Request errors
pub(crate) const LABEL_REQUEST_HYPER: ErrorLabel = ErrorLabel::from_static("request_hyper");

// Handler errors
pub(crate) const LABEL_ABANDONED: ErrorLabel = ErrorLabel::from_static("abandoned");

// Validation errors (granular replacements for generic "validation")
pub(crate) const LABEL_CONTENT_ENCODING_INVALID: ErrorLabel = ErrorLabel::from_static("content_encoding_invalid");
pub(crate) const LABEL_CONTENT_ENCODING_UNSUPPORTED: ErrorLabel = ErrorLabel::from_static("content_encoding_unsupported");
pub(crate) const LABEL_URI_ORIGIN_MISSING: ErrorLabel = ErrorLabel::from_static("uri_origin_missing");
pub(crate) const LABEL_SCHEME_NOT_ALLOWED: ErrorLabel = ErrorLabel::from_static("scheme_not_allowed");
pub(crate) const LABEL_HTTP_VERSION_UNSUPPORTED: ErrorLabel = ErrorLabel::from_static("http_version_unsupported");
pub(crate) const LABEL_TLS: ErrorLabel = ErrorLabel::from_static("tls");

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
    let Some(err) = error.get_ref() else {
        return error.kind().into();
    };

    if let Some(err) = err.downcast_ref::<std::io::Error>() {
        return resolve_io_error_label(err);
    }

    #[cfg(any(feature = "rustls", test))]
    if err.downcast_ref::<rustls::Error>().is_some() {
        return LABEL_TLS;
    }

    #[cfg(any(feature = "native-tls", test))]
    if err.downcast_ref::<native_tls::Error>().is_some() {
        return LABEL_TLS;
    }

    error.kind().into()
}

#[cfg(test)]
mod tests {
    use std::io::{self, ErrorKind};

    use seatbelt::RecoveryInfo;

    use super::*;

    #[derive(Debug)]
    struct DummyError;

    impl std::fmt::Display for DummyError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("dummy")
        }
    }

    impl std::error::Error for DummyError {}

    #[test]
    fn resolves_http_error_label() {
        let err = HttpError::other("boom", RecoveryInfo::never(), LABEL_SCHEME_NOT_ALLOWED);

        let label = resolve_error_label(&err).expect("HttpError should resolve to a label");

        assert_eq!(label, LABEL_SCHEME_NOT_ALLOWED);
    }

    #[test]
    fn resolves_io_error_kind_label() {
        let err = io::Error::from(ErrorKind::ConnectionRefused);

        let label = resolve_error_label(&err).expect("io::Error should resolve to a label");

        assert_eq!(label, ErrorLabel::from(ErrorKind::ConnectionRefused));
    }

    #[test]
    fn resolves_unknown_error_to_none() {
        let err = DummyError;

        assert!(resolve_error_label(&err).is_none());
    }

    #[test]
    fn resolves_io_error_with_inner_kind() {
        // io::Error without a custom inner falls back to its kind.
        let err = io::Error::from(ErrorKind::TimedOut);

        let label = resolve_io_error_label(&err);

        assert_eq!(label, ErrorLabel::from(ErrorKind::TimedOut));
    }

    #[test]
    fn resolves_io_error_with_unrelated_inner_uses_outer_kind() {
        // An io::Error wrapping an arbitrary error falls back to the outer kind.
        let err = io::Error::new(ErrorKind::PermissionDenied, DummyError);

        let label = resolve_io_error_label(&err);

        assert_eq!(label, ErrorLabel::from(ErrorKind::PermissionDenied));
    }

    #[test]
    fn unwraps_nested_io_error() {
        // An io::Error nested inside another io::Error is unwrapped so the
        // innermost kind drives the resulting label.
        let inner = io::Error::from(ErrorKind::ConnectionReset);
        let outer = io::Error::other(inner);

        let label = resolve_io_error_label(&outer);

        assert_eq!(label, ErrorLabel::from(ErrorKind::ConnectionReset));
    }

    #[test]
    fn detects_rustls_error_as_tls_label() {
        let rustls_err = rustls::Error::General("handshake failure".to_owned());
        let io_err = io::Error::other(rustls_err);

        let label = resolve_io_error_label(&io_err);

        assert_eq!(label, LABEL_TLS);
    }

    #[test]
    fn detects_nested_rustls_error_as_tls_label() {
        // A rustls error nested under multiple io::Error layers should still
        // be detected via the recursive unwrap.
        let rustls_err = rustls::Error::General("handshake failure".to_owned());
        let inner = io::Error::other(rustls_err);
        let outer = io::Error::other(inner);

        let label = resolve_io_error_label(&outer);

        assert_eq!(label, LABEL_TLS);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn detects_native_tls_error_as_tls_label() {
        // Trigger a native_tls::Error via a failed pkcs12 parse.
        let Err(native_err) = native_tls::Identity::from_pkcs12(&[], "") else {
            panic!("empty pkcs12 blob must fail to parse")
        };
        let io_err = io::Error::other(native_err);

        let label = resolve_io_error_label(&io_err);

        assert_eq!(label, LABEL_TLS);
    }

    #[test]
    fn collect_error_labels_walks_chain() {
        // The chain combines the outer HttpError label with the inner io::Error label.
        let io_err = io::Error::from(ErrorKind::ConnectionRefused);
        let http_err = HttpError::other(io_err, RecoveryInfo::retry(), LABEL_CONNECT);

        let label = collect_error_labels(&http_err);

        let expected = ErrorLabel::from_parts([LABEL_CONNECT, ErrorLabel::from(ErrorKind::ConnectionRefused)]);
        assert_eq!(label, expected);
    }
}
