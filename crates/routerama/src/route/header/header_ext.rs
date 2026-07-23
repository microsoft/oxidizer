// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use headers::{CacheControl, Date, Expires, HeaderMapExt as _, IfModifiedSince, IfUnmodifiedSince, LastModified};
use http::HeaderMap;

use super::{AcceptEncoding, cache_control};

/// Stateless, strongly-typed accessors for well-known header fields.
///
/// The trait is implemented for [`HeaderMap`] and is [sealed]: it exists to
/// extend the ecosystem `HeaderMap`, not to be implemented by other types.
/// Every method parses on each call; use [`HeaderCache`] to memoize expensive,
/// repeated response date fields.
///
/// [sealed]: https://rust-lang.github.io/api-guidelines/future-proofing.html
pub trait HeaderExt: sealed::Sealed {
    /// Returns the parsed `Date` field, or [`None`] if absent or malformed.
    fn date(&self) -> Option<Date>;

    /// Returns the parsed `Last-Modified` field, or [`None`] if absent or
    /// malformed.
    fn last_modified(&self) -> Option<LastModified>;

    /// Returns the parsed `Expires` field, or [`None`] if absent or malformed.
    fn expires(&self) -> Option<Expires>;

    /// Returns the parsed `If-Modified-Since` field, or [`None`] if absent or
    /// malformed.
    fn if_modified_since(&self) -> Option<IfModifiedSince>;

    /// Returns the parsed `If-Unmodified-Since` field, or [`None`] if absent or
    /// malformed.
    fn if_unmodified_since(&self) -> Option<IfUnmodifiedSince>;

    /// Returns the parsed `Cache-Control` field, or [`None`] if absent.
    ///
    /// Directives from repeated field lines are combined as required by the
    /// field grammar. Directive names are matched case-insensitively, and a
    /// directive that cannot be parsed is ignored without discarding the
    /// directives that can.
    fn cache_control(&self) -> Option<CacheControl>;

    /// Returns the resolved `Accept-Encoding` decision, combining repeated
    /// field lines, or [`None`] if absent or malformed.
    ///
    /// A coding that appears more than once keeps the quality of its last
    /// entry, so a client can narrow an offer it made earlier.
    fn accept_encoding(&self) -> Option<AcceptEncoding>;
}

impl HeaderExt for HeaderMap {
    fn date(&self) -> Option<Date> {
        self.typed_get()
    }

    fn last_modified(&self) -> Option<LastModified> {
        self.typed_get()
    }

    fn expires(&self) -> Option<Expires> {
        self.typed_get()
    }

    fn if_modified_since(&self) -> Option<IfModifiedSince> {
        self.typed_get()
    }

    fn if_unmodified_since(&self) -> Option<IfUnmodifiedSince> {
        self.typed_get()
    }

    fn cache_control(&self) -> Option<CacheControl> {
        let mut values = self.get_all(http::header::CACHE_CONTROL).iter();
        values.next()?;
        Some(cache_control::parse_all(
            self.get_all(http::header::CACHE_CONTROL).iter().map(http::HeaderValue::as_bytes),
        ))
    }

    fn accept_encoding(&self) -> Option<AcceptEncoding> {
        let mut values = self.get_all(http::header::ACCEPT_ENCODING).iter();
        values.next()?;
        AcceptEncoding::parse_all(self.get_all(http::header::ACCEPT_ENCODING).iter().map(http::HeaderValue::as_bytes))
    }
}

mod sealed {
    /// Prevents downstream implementations of [`HeaderExt`](super::HeaderExt).
    pub trait Sealed {}

    impl Sealed for http::HeaderMap {}
}
