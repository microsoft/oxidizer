// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use http::StatusCode;

use crate::resolve_error::ResolveError;
use crate::response::{Body, IntoResponse, Response};

/// A typed failure that prevented an HTTP route handler from being selected.
///
/// Generated `#[fallback]` methods receive this value by value. Path values
/// borrow the request URI and capture names are static route-schema data, so
/// constructing a failure does not allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteFailure<'request> {
    /// No method and path template matched.
    NotFound {
        /// The unmatched request path.
        path: &'request str,
    },
    /// The matcher received a path containing a query or fragment delimiter.
    MalformedPath {
        /// The malformed path text.
        path: &'request str,
    },
    /// A matched route did not yield one of its declared captures.
    MissingCapture {
        /// The affected capture field.
        field: &'static str,
    },
    /// A captured value could not be converted to its declared Rust type.
    InvalidCapture {
        /// The affected capture field.
        field: &'static str,
    },
    /// A captured value was malformed percent-encoding or invalid UTF-8.
    UndecodableCapture {
        /// The affected capture field.
        field: &'static str,
    },
    /// No matching path candidate accepted the request authority.
    HostMismatch {
        /// The matched request path.
        path: &'request str,
    },
    /// No matching path candidate accepted the request content type.
    UnsupportedMediaType {
        /// The matched request path.
        path: &'request str,
    },
    /// No matching path candidate could produce an acceptable representation.
    NotAcceptable {
        /// The matched request path.
        path: &'request str,
    },
}

impl RouteFailure<'_> {
    /// Returns the default HTTP status for this failure.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::NotFound { .. } | Self::HostMismatch { .. } => StatusCode::NOT_FOUND,
            Self::MalformedPath { .. } | Self::MissingCapture { .. } | Self::InvalidCapture { .. } | Self::UndecodableCapture { .. } => {
                StatusCode::BAD_REQUEST
            }
            Self::UnsupportedMediaType { .. } => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::NotAcceptable { .. } => StatusCode::NOT_ACCEPTABLE,
        }
    }

    /// Returns the request path when this failure carries one.
    #[must_use]
    pub const fn path(&self) -> Option<&str> {
        match self {
            Self::NotFound { path }
            | Self::MalformedPath { path }
            | Self::HostMismatch { path }
            | Self::UnsupportedMediaType { path }
            | Self::NotAcceptable { path } => Some(path),
            Self::MissingCapture { .. } | Self::InvalidCapture { .. } | Self::UndecodableCapture { .. } => None,
        }
    }

    /// Returns the affected capture field, when capture conversion failed.
    #[must_use]
    pub const fn field(&self) -> Option<&'static str> {
        match self {
            Self::MissingCapture { field } | Self::InvalidCapture { field } | Self::UndecodableCapture { field } => Some(field),
            Self::NotFound { .. }
            | Self::MalformedPath { .. }
            | Self::HostMismatch { .. }
            | Self::UnsupportedMediaType { .. }
            | Self::NotAcceptable { .. } => None,
        }
    }
}

impl fmt::Display for RouteFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "no route matched path `{path}`"),
            Self::MalformedPath { path } => write!(f, "expected a URI path without a query or fragment, got `{path}`"),
            Self::MissingCapture { field } => write!(f, "missing capture for field `{field}`"),
            Self::InvalidCapture { field } => write!(f, "failed to parse capture for field `{field}`"),
            Self::UndecodableCapture { field } => write!(f, "failed to percent-decode capture for field `{field}`"),
            Self::HostMismatch { path } => write!(f, "no route candidate for `{path}` accepted the request host"),
            Self::UnsupportedMediaType { path } => {
                write!(f, "no route candidate for `{path}` accepted the request content type")
            }
            Self::NotAcceptable { path } => {
                write!(f, "no route candidate for `{path}` could produce an acceptable representation")
            }
        }
    }
}

impl core::error::Error for RouteFailure<'_> {}

impl IntoResponse for RouteFailure<'_> {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        self.status().into_response()
    }
}

pub(crate) const fn from_resolve_error(error: ResolveError<'_>) -> RouteFailure<'_> {
    match error {
        ResolveError::NotFound(path) => RouteFailure::NotFound { path },
        ResolveError::InvalidPath(path) => RouteFailure::MalformedPath { path },
        ResolveError::MissingCapture(field) => RouteFailure::MissingCapture { field },
        ResolveError::InvalidCapture(field) => RouteFailure::InvalidCapture { field },
        ResolveError::UndecodableCapture(field) => RouteFailure::UndecodableCapture { field },
    }
}
