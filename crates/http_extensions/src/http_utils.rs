// Copyright (c) Microsoft Corporation.

use http::{HeaderName, HeaderValue};

#[cfg(any(feature = "json", test))]
pub(crate) const CONTENT_TYPE_JSON: HeaderValue = HeaderValue::from_static("application/json");

pub(crate) const CONTENT_TYPE_TEXT: HeaderValue = HeaderValue::from_static("text/plain");

pub(crate) const CONTENT_LENGTH_ZERO: HeaderValue = HeaderValue::from_static("0");

/// A workaround to allow mutable access to headers in both request and response builders.
pub(crate) trait HeadersBuilder {
    fn headers_mut(&mut self) -> Option<&mut http::header::HeaderMap>;
}

impl HeadersBuilder for http::request::Builder {
    fn headers_mut(&mut self) -> Option<&mut http::header::HeaderMap> {
        self.headers_mut()
    }
}

impl HeadersBuilder for http::response::Builder {
    fn headers_mut(&mut self) -> Option<&mut http::header::HeaderMap> {
        self.headers_mut()
    }
}

#[cfg_attr(test, mutants::skip)] // One match arm is for optimization, so mutation has no testable effect.
pub(crate) fn try_content_length_header(
    builder: &mut impl HeadersBuilder,
    content_length: u64,
) -> bool {
    if content_length == 0 {
        try_header(builder, http::header::CONTENT_LENGTH, CONTENT_LENGTH_ZERO)
    } else {
        try_header(builder, http::header::CONTENT_LENGTH, content_length)
    }
}

/// Tries to set a header on the given request builder if headers is not yet set.
///
/// Returns true if the header was set, false otherwise.
pub(crate) fn try_header(
    builder: &mut impl HeadersBuilder,
    key: HeaderName,
    value: impl Into<HeaderValue>,
) -> bool {
    if let Some(headers) = builder.headers_mut()
        && let http::header::Entry::Vacant(vacant_entry) = headers.entry(key)
    {
        vacant_entry.insert(value.into());
        return true;
    }
    false
}

/// A holder for a value of type T that is `Sync` even if T is not `Sync`.
///
/// This works because the inner T can never be accessed from the holder. The
/// only way to get the inner T is to consume the `SyncHolder<T>` itself.
#[derive(Debug)]
pub(crate) struct SyncHolder<T> {
    value: T,
}

// NOTE: Do not add any methods that would expose &T or &mut T references.
// The only way to get the inner T is to consume the `SyncHolder<T>` itself.
// This is important to ensure that `SyncHolder<T>` is Sync and no invariants are violated.
impl<T> SyncHolder<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

// SAFETY: SyncHolder<T> is Sync because at no point can the inner T be accessed.
// The only way to get the inner T is to consume the SyncHolder<T> itself.
unsafe impl<T> Sync for SyncHolder<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpRequest;

    static_assertions::assert_impl_all!(SyncHolder<HttpRequest>: Send, Sync);
    static_assertions::assert_not_impl_all!(SyncHolder<bool>: Clone);

    #[test]
    fn content_length_0() {
        let mut builder = http::request::Builder::new();
        let was_set = try_content_length_header(&mut builder, 0);
        assert!(was_set);
        let request = builder.body(()).unwrap();
        let header_ref = request.headers().get(http::header::CONTENT_LENGTH).unwrap();
        assert_eq!(header_ref, &CONTENT_LENGTH_ZERO);
        assert_eq!(header_ref.as_bytes(), CONTENT_LENGTH_ZERO.as_bytes());
        // For some reason, they have different addresses, is something broken?
        // assert_eq!(header_ref.as_bytes().as_ptr(), CONTENT_LENGTH_ZERO.as_bytes().as_ptr());
    }

    #[test]
    fn content_length_non_0() {
        let mut builder = http::request::Builder::new();
        let was_set = try_content_length_header(&mut builder, 1234);
        assert!(was_set);
        let request = builder.body(()).unwrap();
        let header_ref = request.headers().get(http::header::CONTENT_LENGTH).unwrap();
        assert_eq!(header_ref, "1234");
    }

    #[test]
    fn content_length_overwrite() {
        let mut builder = http::request::Builder::new();
        let was_set1 = try_content_length_header(&mut builder, 123);
        assert!(was_set1);
        let was_set2 = try_content_length_header(&mut builder, 1234);
        assert!(!was_set2);
        let request = builder.body(()).unwrap();
        let header_ref = request.headers().get(http::header::CONTENT_LENGTH).unwrap();
        assert_eq!(header_ref, "123");
    }
}
