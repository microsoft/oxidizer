// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The label of an error and its source chain.

use ohno::{ErrorLabel, Labeled};

use crate::HttpError;

/// Returns the labels of `error` and of the errors in its source chain, joined with dots.
///
/// The chain starts at `error` and follows [`Error::source`](std::error::Error::source). The
/// function takes the label of each [`HttpError`] and the kind of each [`std::io::Error`], and
/// skips other errors. If it finds no label, the result is empty.
///
/// For an I/O error that wraps another I/O error, the function uses the inner error.
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

    error.kind().into()
}
