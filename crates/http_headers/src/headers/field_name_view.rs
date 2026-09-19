// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::hash::{Hash, Hasher};

use crate::headers::tokens::header_name_text;
use crate::{FieldName, InvalidFieldName, validate};

/// A borrowed field-name token with ASCII-case-insensitive equality and hashing.
///
/// The original spelling is preserved. Unlike the owned [`FieldName`], this
/// view does not impose a length limit or allocate for extension names.
///
/// Available with either the `headers-cors` or `headers-negotiation` feature.
///
/// # Examples
///
/// ```
/// use http_headers::headers::FieldNameView;
///
/// let name = FieldNameView::new("X-Trace-Id")?;
/// assert_eq!(name, FieldNameView::new("x-trace-id")?);
/// assert_eq!(name.as_str(), "X-Trace-Id");
/// assert_eq!(name.try_to_field_name()?.as_str(), "x-trace-id");
/// # Ok::<(), http_headers::InvalidFieldName>(())
/// ```
#[derive(Clone, Copy)]
pub struct FieldNameView<'a>(&'a [u8]);

impl<'a> FieldNameView<'a> {
    /// Validates and borrows a field-name token without allocating.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidFieldName`] for empty input or non-token bytes.
    #[inline]
    pub fn new(name: &'a str) -> Result<Self, InvalidFieldName> {
        if validate::token(name.as_bytes()) {
            Ok(Self(name.as_bytes()))
        } else {
            Err(InvalidFieldName)
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
        header_name_text(self.0)
    }

    /// Returns the original bytes.
    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    /// Compares to another spelling without allocating or normalizing storage.
    #[must_use]
    #[inline]
    pub fn eq_ignore_ascii_case(self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other.as_bytes())
    }

    /// Materializes an owned field name, normalizing its spelling.
    ///
    /// Known names do not allocate; extension names may allocate.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidFieldName`] if the token exceeds the owned type's
    /// 65,535-byte length limit.
    #[inline]
    pub fn try_to_field_name(self) -> Result<FieldName, InvalidFieldName> {
        FieldName::try_from_bytes(self.as_bytes())
    }

    /// Materializes an external field name, normalizing its spelling.
    ///
    /// # Errors
    ///
    /// Returns the external constructor's error if its constraints reject
    /// the token. This conversion may allocate.
    #[cfg(feature = "http")]
    #[inline]
    pub fn try_to_http_header_name(self) -> Result<http::HeaderName, http::header::InvalidHeaderName> {
        http::HeaderName::from_bytes(self.as_bytes())
    }
}

impl<'a> TryFrom<&'a str> for FieldNameView<'a> {
    type Error = InvalidFieldName;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl PartialEq for FieldNameView<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.0)
    }
}

impl Eq for FieldNameView<'_> {}

impl Hash for FieldNameView<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.len().hash(state);
        for byte in self.0 {
            state.write_u8(byte.to_ascii_lowercase());
        }
    }
}

impl AsRef<str> for FieldNameView<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for FieldNameView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for FieldNameView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FieldNameView").field(&self.as_str()).finish()
    }
}
