// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The trait a field container implements to supply field lines.

use crate::FieldName;
use crate::source::FieldLines;

/// A container that supplies stored HTTP field lines.
///
/// This trait is implemented for individual providers of HTTP fields and is
/// how this crate acquires fields to parse.
///
/// You can use the `http` cargo feature to get an implementation of this trait for the common
/// [`HeaderMap`](https://docs.rs/http/latest/http/header/struct.HeaderMap.html) type.
///
/// Names at this boundary must be static descriptors. Custom descriptors can
/// use a `static LazyLock<FieldName>`; locally constructed runtime names cannot
/// be passed to this trait. Use the container's native API for dynamic lookup.
/// [`FieldName::try_from_bytes`] remains available for validating, comparing,
/// and converting runtime names.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # {
/// use http::HeaderMap;
/// use http_headers::FieldName;
/// use http_headers::source::FieldSource;
///
/// assert!(FieldSource::lines(&HeaderMap::new(), &FieldName::UserAgent).is_none());
/// # }
/// ```
pub trait FieldSource {
    /// Returns whether any field line is stored under `name`.
    #[inline]
    fn contains(&self, name: &'static FieldName) -> bool {
        self.lines(name).is_some()
    }

    /// Returns the field lines stored under `name`.
    ///
    /// `None` means the field is absent. `Some` always contains at least one
    /// raw field line; a present zero-length field line is therefore distinct
    /// from absence.
    ///
    /// Borrowed and owned decoding from custom sources accept at most
    /// [`MAX_CUSTOM_FIELD_BYTES`](crate::source::MAX_CUSTOM_FIELD_BYTES) total
    /// bytes and [`MAX_CUSTOM_FIELD_LINES`](crate::source::MAX_CUSTOM_FIELD_LINES)
    /// lines for one name. Delimited parsing accepts at most
    /// [`MAX_CUSTOM_LIST_ITEMS`](crate::source::MAX_CUSTOM_LIST_ITEMS) items.
    /// Exceeding a limit returns
    /// [`DecodeErrorKind::SourceLimitExceeded`](crate::DecodeErrorKind::SourceLimitExceeded).
    /// The native `http::HeaderMap` representation is exempt; a custom source
    /// backed by validated [`FieldValue`](crate::FieldValue) values is not.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # {
    /// use http::HeaderMap;
    /// use http_headers::FieldName;
    /// use http_headers::source::FieldSource;
    ///
    /// assert!(FieldSource::lines(&HeaderMap::new(), &FieldName::UserAgent).is_none());
    /// # }
    /// ```
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>>;
}

impl<S> FieldSource for &S
where
    S: FieldSource + ?Sized,
{
    #[inline]
    fn contains(&self, name: &'static FieldName) -> bool {
        (**self).contains(name)
    }

    #[inline]
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (**self).lines(name)
    }
}
