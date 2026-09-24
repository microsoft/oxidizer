// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::InvalidMethod;
use crate::headers::tokens::method_text;
use crate::validate;

/// A borrowed HTTP method token with case-sensitive equality and hashing.
///
/// Extension methods retain their spelling. `*` is a method token, not a
/// wildcard, and `get` differs from [`Self::GET`].
///
/// Available with either the `headers-cors` or `headers-negotiation` feature.
///
/// # Examples
///
/// ```
/// use http_headers::headers::MethodView;
///
/// assert_ne!(MethodView::GET, MethodView::new("get")?);
/// assert_eq!(MethodView::new("X-CUSTOM")?.as_str(), "X-CUSTOM");
/// # Ok::<(), http_headers::headers::InvalidMethod>(())
/// ```
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MethodView<'a>(&'a [u8]);

impl<'a> MethodView<'a> {
    /// The `GET` method.
    pub const GET: Self = Self(b"GET");
    /// The `HEAD` method.
    pub const HEAD: Self = Self(b"HEAD");
    /// The `POST` method.
    pub const POST: Self = Self(b"POST");
    /// The `PUT` method.
    pub const PUT: Self = Self(b"PUT");
    /// The `DELETE` method.
    pub const DELETE: Self = Self(b"DELETE");
    /// The `CONNECT` method.
    pub const CONNECT: Self = Self(b"CONNECT");
    /// The `OPTIONS` method.
    pub const OPTIONS: Self = Self(b"OPTIONS");
    /// The `TRACE` method.
    pub const TRACE: Self = Self(b"TRACE");
    /// The `PATCH` method.
    pub const PATCH: Self = Self(b"PATCH");

    /// Validates and borrows a method without allocating.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMethod`] for empty input or non-token bytes.
    #[inline]
    pub fn new(method: &'a str) -> Result<Self, InvalidMethod> {
        if validate::token(method.as_bytes()) {
            Ok(Self(method.as_bytes()))
        } else {
            Err(InvalidMethod)
        }
    }

    #[inline]
    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    /// Returns the original spelling.
    #[must_use]
    #[inline]
    pub fn as_str(self) -> &'a str {
        method_text(self.0)
    }

    /// Returns the original bytes.
    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    /// Converts to an owned [`http::Method`].
    ///
    /// Standard methods do not allocate. Extension methods may allocate and
    /// are checked against the external type's own constraints.
    ///
    /// # Errors
    ///
    /// Returns the external constructor's error if it rejects the token.
    #[cfg(feature = "http")]
    #[inline]
    pub fn try_to_method(self) -> Result<http::Method, http::method::InvalidMethod> {
        http::Method::from_bytes(self.as_bytes())
    }
}

impl<'a> TryFrom<&'a str> for MethodView<'a> {
    type Error = InvalidMethod;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl AsRef<str> for MethodView<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for MethodView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for MethodView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MethodView").field(&self.as_str()).finish()
    }
}
