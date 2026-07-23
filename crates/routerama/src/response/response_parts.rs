// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::{Extensions, HeaderMap, StatusCode, Version};

/// The metadata being composed around a response body.
///
/// This type deliberately exposes status, version, headers, and extensions,
/// but not the response body. An [`IntoResponseParts`] implementation returns
/// the value on success. On failure, tuple conversion discards this value and
/// the original success body before converting the typed rejection.
#[derive(Debug)]
pub struct ResponseParts {
    inner: http::response::Parts,
}

impl ResponseParts {
    pub(super) fn new(inner: http::response::Parts) -> Self {
        Self { inner }
    }

    pub(super) fn into_inner(self) -> http::response::Parts {
        self.inner
    }

    /// Returns the response status.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.inner.status
    }

    /// Returns mutable access to the response status.
    #[must_use]
    pub fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.inner.status
    }

    /// Returns the HTTP version.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.inner.version
    }

    /// Returns mutable access to the HTTP version.
    #[must_use]
    pub fn version_mut(&mut self) -> &mut Version {
        &mut self.inner.version
    }

    /// Returns the response headers.
    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.inner.headers
    }

    /// Returns mutable access to the response headers.
    #[must_use]
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.inner.headers
    }

    /// Returns the response extensions.
    #[must_use]
    pub const fn extensions(&self) -> &Extensions {
        &self.inner.extensions
    }

    /// Returns mutable access to the response extensions.
    #[must_use]
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.inner.extensions
    }
}
