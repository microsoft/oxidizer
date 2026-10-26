// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use http::HeaderValue;
use http::header::InvalidHeaderValue;

/// Creates [`HeaderValue`] values from shared byte buffers.
///
/// This extension accepts types that convert to [`bytes::Bytes`].
/// It delegates construction to [`HeaderValue::from_maybe_shared`].
/// The operation does not copy data when the input permits sharing.
pub trait HeaderValueExt: sealed::Sealed {
    /// Creates a [`HeaderValue`] from a source convertible to [`Bytes`].
    ///
    /// [`HeaderValue::from_maybe_shared`] rejects invalid header bytes.
    /// It does not copy a source that uses one contiguous memory slice.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidHeaderValue`] if the byte sequence contains invalid
    /// header value bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use bytesbuf::BytesView;
    /// use bytesbuf::mem::GlobalPool;
    /// use http::HeaderValue;
    /// use http_extensions::HeaderValueExt;
    ///
    /// let view = BytesView::copied_from_slice(b"application/json", &GlobalPool::new());
    /// let value = HeaderValue::from_shared(view.to_bytes()).unwrap();
    /// assert_eq!(value, "application/json");
    /// ```
    fn from_shared(src: impl Into<Bytes>) -> Result<HeaderValue, InvalidHeaderValue>;
}

impl HeaderValueExt for HeaderValue {
    fn from_shared(src: impl Into<Bytes>) -> Result<HeaderValue, InvalidHeaderValue> {
        Self::from_maybe_shared(src.into())
    }
}

mod sealed {
    use http::HeaderValue;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl Sealed for HeaderValue {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use http::HeaderValue;

    use crate::HeaderValueExt;

    #[test]
    fn from_shared_valid_ascii() {
        let view = BytesView::copied_from_slice(b"text/plain", &GlobalPool::new());
        let value = HeaderValue::from_shared(view.to_bytes()).unwrap();
        assert_eq!(value, "text/plain");
    }

    #[test]
    fn from_shared_valid_opaque_bytes() {
        let view = BytesView::copied_from_slice(b"hello\x80\xff", &GlobalPool::new());
        let value = HeaderValue::from_shared(view.to_bytes()).unwrap();
        assert_eq!(value.as_bytes(), b"hello\x80\xff");
    }

    #[test]
    fn from_shared_empty() {
        let view = BytesView::new();
        let value = HeaderValue::from_shared(view.to_bytes()).unwrap();
        assert_eq!(value, "");
        assert!(value.is_empty());
    }

    #[test]
    fn from_shared_rejects_invalid_bytes() {
        let view = BytesView::copied_from_slice(b"bad\x00value", &GlobalPool::new());
        HeaderValue::from_shared(view.to_bytes()).unwrap_err();
    }

    #[test]
    fn from_shared_rejects_newline() {
        let view = BytesView::copied_from_slice(b"line1\nline2", &GlobalPool::new());
        HeaderValue::from_shared(view.to_bytes()).unwrap_err();
    }
}
