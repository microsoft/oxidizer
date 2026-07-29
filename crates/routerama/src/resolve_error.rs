// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

/// An error returned when resolving an HTTP method and path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolveError<'p> {
    /// The input contained a query (`?`) or fragment (`#`) delimiter and was
    /// therefore not a URI path.
    InvalidPath(&'p str),

    /// No static or dynamic route matched the request.
    ///
    /// Contains the unmatched request path.
    NotFound(&'p str),

    /// A required capture was absent from a matched route.
    MissingCapture(&'static str),

    /// A captured value could not be parsed into its field type.
    InvalidCapture(&'static str),

    /// A captured value contained malformed percent encoding or decoded to
    /// invalid UTF-8.
    UndecodableCapture(&'static str),
}

impl<'p> ResolveError<'p> {
    /// Returns the request path for path-related errors.
    #[must_use]
    pub fn path(&self) -> Option<&'p str> {
        match self {
            Self::InvalidPath(path) | Self::NotFound(path) => Some(path),
            Self::MissingCapture(_) | Self::InvalidCapture(_) | Self::UndecodableCapture(_) => None,
        }
    }

    /// Returns the affected capture field, if capture extraction failed.
    #[must_use]
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Self::InvalidPath(_) | Self::NotFound(_) => None,
            Self::MissingCapture(field) | Self::InvalidCapture(field) | Self::UndecodableCapture(field) => Some(field),
        }
    }
}

impl fmt::Display for ResolveError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(f, "expected a URI path without a query or fragment, got `{path}`"),
            Self::NotFound(path) => write!(f, "no route matched path `{path}`"),
            Self::MissingCapture(field) => write!(f, "missing capture for field `{field}`"),
            Self::InvalidCapture(field) => write!(f, "failed to parse capture for field `{field}`"),
            Self::UndecodableCapture(field) => write!(f, "failed to percent-decode capture for field `{field}`"),
        }
    }
}

impl core::error::Error for ResolveError<'_> {}
