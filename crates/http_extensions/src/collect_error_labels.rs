// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The label of an error and its source chain.

use ohno::{ErrorLabel, Labeled};

use crate::HttpError;
#[cfg(any(feature = "rustls", feature = "native-tls", test))]
use crate::error_labels::LABEL_TLS;

/// Returns the labels of `error` and of the errors in its source chain, joined with dots.
///
/// The chain starts at `error` and follows [`Error::source`](std::error::Error::source). The
/// function takes the label of each [`HttpError`] and the kind of each [`std::io::Error`], and
/// skips other errors. If it finds no label, the result is empty.
///
/// For an I/O error that wraps another I/O error, the function uses the inner error. With the
/// `rustls` or `native-tls` feature, an I/O error that wraps an error of that TLS library gets the
/// label `tls`.
///
/// # Examples
///
/// ```
/// use std::io::{Error, ErrorKind};
///
/// use http_extensions::{HttpError, collect_error_labels};
/// use recoverable::RecoveryInfo;
///
/// let refused = Error::from(ErrorKind::ConnectionRefused);
/// let error = HttpError::other(refused, RecoveryInfo::retry(), "connect");
///
/// assert_eq!(collect_error_labels(&error), "connect.connection_refused");
/// ```
#[must_use]
pub fn collect_error_labels(error: &(dyn std::error::Error + 'static)) -> ErrorLabel {
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn detects_rustls_error_as_tls_label() {
        let rustls_err = rustls::Error::General("handshake failure".to_owned());
        let io_err = io::Error::other(rustls_err);

        let label = resolve_io_error_label(&io_err);

        assert_eq!(label, LABEL_TLS);
    }

    #[test]
    fn detects_nested_rustls_error_as_tls_label() {
        // Recursive unwrapping finds a rustls error under several io::Error layers.
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
}
