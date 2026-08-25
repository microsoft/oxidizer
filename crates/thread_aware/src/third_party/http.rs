// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`http`](::http) (1.x) types.
//!
//! Enable with the `http` Cargo feature.
//!
//! Inert value types (`StatusCode`, `Method`, `Version`, `HeaderName`,
//! `HeaderValue`, and the URI components `Uri`, `Authority`, `Scheme`,
//! `PathAndQuery`, `Port<T>`, plus the error types `Error`, `InvalidUri`)
//! get a no-op `relocate`. The `HeaderMap` impl is provided
//! for `HeaderMap<HeaderValue>` only (the default type produced
//! by the `http` crate) and is also a no-op, since `HeaderValue::relocate`
//! is itself no-op — iterating would be pure waste. See the per-impl docs
//! on [`Request<T>`] and [`Response<T>`] for their `relocate` semantics.

use ::http::header::{HeaderMap, HeaderName, HeaderValue};
use ::http::uri::{Authority, InvalidUri, PathAndQuery, Port, Scheme};
use ::http::{Error, Method, Request, Response, StatusCode, Uri, Version};

use crate::ThreadAware;
use crate::affinity::Affinity;

impl_noop_thread_aware!(
    StatusCode,
    Version,
    Method,
    HeaderName,
    HeaderValue,
    HeaderMap<HeaderValue>,
    Uri,
    Authority,
    Scheme,
    PathAndQuery,
    Error,
    InvalidUri,
);

/// `Port<T>` is inert regardless of its representation parameter `T`:
/// the port number is stored as a `u16` and `T` is only used for the textual
/// representation accessor. `relocate` is therefore a no-op for all `T`.
impl<T: Send> ThreadAware for Port<T> {
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
}

/// `relocate` is forwarded to the body only.
///
/// Headers (`HeaderMap<HeaderValue>`) are inert per the no-op impl in this
/// module, so iterating them would be wasted work. `http::Extensions` holds
/// arbitrary `Any` values whose concrete types are erased at runtime, so this
/// impl cannot relocate them — callers that stash thread-affine state in
/// extensions must relocate it explicitly.
impl<T: ThreadAware> ThreadAware for Request<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.body_mut().relocate(source, destination);
    }
}

/// `relocate` is forwarded to the body only.
///
/// Headers (`HeaderMap<HeaderValue>`) are inert per the no-op impl in this
/// module, so iterating them would be wasted work. `http::Extensions` holds
/// arbitrary `Any` values whose concrete types are erased at runtime, so this
/// impl cannot relocate them — callers that stash thread-affine state in
/// extensions must relocate it explicitly.
impl<T: ThreadAware> ThreadAware for Response<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.body_mut().relocate(source, destination);
    }
}

#[cfg(test)]
mod tests {
    use ::http::header::{HeaderMap, HeaderName, HeaderValue};
    use ::http::uri::{Authority, InvalidUri, PathAndQuery, Port, Scheme};
    use ::http::{Error, Method, Request, Response, StatusCode, Uri, Version};
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    use crate::affinity::{Affinity, pinned_affinities};

    /// Counts how many times `relocate` has been called.
    ///
    /// Used by mutation-testing-killing tests to detect that `Request<T>` and
    /// `Response<T>` actually delegate to their inner body rather than no-op'ing.
    /// Without this, mutants that replace the body of `relocate` with `()` would
    /// survive when the body type is something like `Vec<u8>` whose own
    /// `relocate` has no observable effect.
    #[derive(Default)]
    struct Counter(u32);

    impl ThreadAware for Counter {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.0 += 1;
        }
    }

    assert_impl_all!(StatusCode: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Version: ThreadAware, Send, Sync, Copy);
    assert_impl_all!(Method: ThreadAware, Send, Sync);
    assert_impl_all!(HeaderName: ThreadAware, Send, Sync);
    assert_impl_all!(HeaderValue: ThreadAware, Send, Sync);
    assert_impl_all!(HeaderMap<HeaderValue>: ThreadAware, Send, Sync);
    assert_impl_all!(Uri: ThreadAware, Send, Sync);
    assert_impl_all!(Authority: ThreadAware, Send, Sync);
    assert_impl_all!(Scheme: ThreadAware, Send, Sync);
    assert_impl_all!(PathAndQuery: ThreadAware, Send, Sync);
    assert_impl_all!(Port<&'static str>: ThreadAware, Send, Sync);
    assert_impl_all!(Error: ThreadAware, Send, Sync);
    assert_impl_all!(InvalidUri: ThreadAware, Send, Sync);
    assert_impl_all!(Request<Vec<u8>>: ThreadAware, Send, Sync);
    assert_impl_all!(Response<Vec<u8>>: ThreadAware, Send, Sync);

    #[test]
    fn method_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value = Method::POST;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, Method::POST);
    }

    #[test]
    fn uri_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut uri: Uri = "https://example.com:8443/path?q=1".parse().unwrap();
        uri.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(uri.to_string(), "https://example.com:8443/path?q=1");
    }

    #[test]
    fn authority_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut authority: Authority = "example.org:80".parse().unwrap();
        authority.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(authority.as_str(), "example.org:80");
    }

    #[test]
    fn scheme_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut scheme: Scheme = "https".parse().unwrap();
        scheme.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(scheme.as_str(), "https");
    }

    #[test]
    fn path_and_query_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut pq: PathAndQuery = "/foo/bar?baz=1".parse().unwrap();
        pq.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(pq.as_str(), "/foo/bar?baz=1");
    }

    #[test]
    fn port_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let authority: Authority = "example.org:8080".parse().unwrap();
        let mut port = authority.port().unwrap();
        port.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(port.as_u16(), 8080);
    }

    #[test]
    fn error_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        // `http::Error` cannot be constructed directly; obtain one via a
        // failing conversion that bubbles up through `http::Error: From<_>`.
        let invalid: InvalidUri = "::not a uri::".parse::<Uri>().unwrap_err();
        let mut err: Error = invalid.into();
        err.relocate(Some(affinities[0]), affinities[1]);
        // Just assert the value survives the call.
        let _ = err.to_string();
    }

    #[test]
    fn invalid_uri_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut invalid: InvalidUri = "::not a uri::".parse::<Uri>().unwrap_err();
        invalid.relocate(Some(affinities[0]), affinities[1]);
        let _ = invalid.to_string();
    }

    #[test]
    fn header_map_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut map: HeaderMap<HeaderValue> = HeaderMap::new();
        map.insert("x-one", HeaderValue::from_static("one"));
        map.insert("x-two", HeaderValue::from_static("two"));
        map.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(map.get("x-one").unwrap(), "one");
        assert_eq!(map.get("x-two").unwrap(), "two");
    }

    #[test]
    fn request_relocate_propagates_to_body() {
        let affinities = pinned_affinities(&[2]);
        let mut req = Request::new(Counter::default());
        req.headers_mut()
            .insert(HeaderName::from_static("x-trace"), HeaderValue::from_static("abc"));
        req.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(req.body().0, 1);
        assert_eq!(req.headers().get("x-trace").unwrap(), "abc");
    }

    #[test]
    fn response_relocate_propagates_to_body() {
        let affinities = pinned_affinities(&[2]);
        let mut resp = Response::new(Counter::default());
        resp.headers_mut()
            .insert(HeaderName::from_static("x-trace"), HeaderValue::from_static("xyz"));
        resp.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(resp.body().0, 1);
        assert_eq!(resp.headers().get("x-trace").unwrap(), "xyz");
    }
}
