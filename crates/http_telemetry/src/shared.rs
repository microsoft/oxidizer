// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The values of the fields that the events of both sides record.

use std::error::Error;
use std::sync::Arc;

use http::{Method, Version};
use http_extensions::HttpError;
use observed::Text;
use ohno::{ErrorLabel, Labeled};

/// The value that the events record in place of an unknown HTTP method or protocol version, an unknown URL scheme on
/// the server, or a missing URL scheme on the client. Recording each unknown value as it is would add a metric series
/// for each value.
pub(crate) const OTHER: &str = "_OTHER";

/// The value of `http.request.method` for `method`.
pub(crate) const fn http_method_name(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => OTHER,
    }
}

/// The value of `http.request.method_original`: the text of `method`, if `http.request.method` records `_OTHER`.
pub(crate) fn method_original(method: &Method) -> Option<Text> {
    (http_method_name(method) == OTHER).then(|| Text::from(Arc::<str>::from(method.as_str())))
}

/// The value of `network.protocol.version` for `version`.
pub(crate) const fn protocol_version(version: Version) -> &'static str {
    match version {
        Version::HTTP_10 => "1.0",
        Version::HTTP_11 => "1.1",
        Version::HTTP_2 => "2",
        Version::HTTP_3 => "3",
        _ => OTHER,
    }
}

/// The value of `url.scheme` and the default port of the schemes `http` and `https`, or `None` for any other scheme.
pub(crate) fn known_scheme(scheme: &str) -> Option<(&'static str, u16)> {
    match scheme {
        "http" => Some(("http", 80)),
        "https" => Some(("https", 443)),
        _ => None,
    }
}

/// The labels of `error` and its sources, joined with dots: the label of each [`HttpError`] and the kind of each I/O
/// error.
pub(crate) fn collect_error_labels(error: &(dyn Error + 'static)) -> ErrorLabel {
    ErrorLabel::from_error_chain(error, resolve_error_label)
}

/// The label of one error of a source chain: the label of an [`HttpError`], the kind of an I/O
/// error, or none for any other error.
fn resolve_error_label(error: &(dyn Error + 'static)) -> Option<ErrorLabel> {
    if let Some(error) = error.downcast_ref::<HttpError>() {
        return Some(error.label().clone());
    }
    error.downcast_ref::<std::io::Error>().map(resolve_io_error_label)
}

/// The kind of the I/O error that `error` wraps, if it wraps one, else the kind of `error`.
fn resolve_io_error_label(error: &std::io::Error) -> ErrorLabel {
    match error.get_ref().and_then(|inner| inner.downcast_ref::<std::io::Error>()) {
        Some(inner) => resolve_io_error_label(inner),
        None => error.kind().into(),
    }
}
