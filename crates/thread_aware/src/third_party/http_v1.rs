// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`http`] (1.x) types.
//!
//! Enable with the `http_v1` Cargo feature.
//!
//! Inert value types (`StatusCode`, `Method`, `Version`, `HeaderName`,
//! `HeaderValue`) get a no-op `relocate`. Container types (`HeaderMap<T>`,
//! `Request<T>`, `Response<T>`) propagate `relocate` to their headers and (for
//! `Request`/`Response`) their body, mirroring how this crate handles `Vec<T>`
//! and `Box<T>`.
//!
//! Note: `http::Extensions` (carried by `Request<T>` and `Response<T>`) holds
//! arbitrary `Any` values whose concrete types are erased at runtime, so this
//! impl cannot relocate them. Callers that stash thread-affine state in
//! extensions must relocate it explicitly.

use ::http::header::{HeaderMap, HeaderName, HeaderValue};
use ::http::{Method, Request, Response, StatusCode, Version};

use crate::ThreadAware;
use crate::affinity::Affinity;

impl_noop_thread_aware!(StatusCode, Version, Method, HeaderName, HeaderValue);

impl<T: ThreadAware> ThreadAware for HeaderMap<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T: ThreadAware> ThreadAware for Request<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        for value in self.headers_mut().values_mut() {
            value.relocate(source, destination);
        }
        self.body_mut().relocate(source, destination);
    }
}

impl<T: ThreadAware> ThreadAware for Response<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        for value in self.headers_mut().values_mut() {
            value.relocate(source, destination);
        }
        self.body_mut().relocate(source, destination);
    }
}

#[cfg(test)]
mod tests {
    use ::http::header::{HeaderMap, HeaderName, HeaderValue};
    use ::http::{Method, Request, Response, StatusCode, Version};
    use static_assertions::assert_impl_all;

    use crate::ThreadAware;
    use crate::affinity::Affinity;
    #[cfg(feature = "threads")]
    use crate::affinity::pinned_affinities;

    /// Counts how many times `relocate` has been called.
    ///
    /// Used by mutation-testing-killing tests to detect that container impls
    /// actually delegate to their inner elements rather than no-op'ing —
    /// otherwise mutants that replace the body of `relocate` with `()` would
    /// survive when the inner type's `relocate` itself has no observable
    /// effect (as it does for `HeaderValue` or `Vec<u8>`).
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
    assert_impl_all!(Request<Vec<u8>>: ThreadAware, Send, Sync);
    assert_impl_all!(Response<Vec<u8>>: ThreadAware, Send, Sync);

    #[test]
    #[cfg(feature = "threads")]
    fn method_relocate_is_noop() {
        let affinities = pinned_affinities(&[2]);
        let mut value = Method::POST;
        value.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(value, Method::POST);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn header_map_relocate_propagates_to_values() {
        let affinities = pinned_affinities(&[2]);
        let mut map: HeaderMap<HeaderValue> = HeaderMap::new();
        map.insert("x-one", HeaderValue::from_static("one"));
        map.insert("x-two", HeaderValue::from_static("two"));
        map.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(map.get("x-one").unwrap(), "one");
        assert_eq!(map.get("x-two").unwrap(), "two");
    }

    #[test]
    #[cfg(feature = "threads")]
    fn request_relocate_propagates_to_body_and_headers() {
        let affinities = pinned_affinities(&[2]);
        let mut req = Request::new(vec![1_u8, 2, 3]);
        req.headers_mut().insert("x-trace", HeaderValue::from_static("abc"));
        req.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(req.body(), &vec![1_u8, 2, 3]);
        assert_eq!(req.headers().get("x-trace").unwrap(), "abc");
    }

    #[test]
    #[cfg(feature = "threads")]
    fn response_relocate_propagates_to_body_and_headers() {
        let affinities = pinned_affinities(&[2]);
        let mut resp = Response::new(vec![4_u8, 5, 6]);
        resp.headers_mut().insert("x-trace", HeaderValue::from_static("xyz"));
        resp.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(resp.body(), &vec![4_u8, 5, 6]);
        assert_eq!(resp.headers().get("x-trace").unwrap(), "xyz");
    }

    #[test]
    #[cfg(feature = "threads")]
    fn header_map_relocate_calls_relocate_on_every_value() {
        let affinities = pinned_affinities(&[2]);
        let mut map: HeaderMap<Counter> = HeaderMap::default();
        map.insert(HeaderName::from_static("x-one"), Counter::default());
        map.insert(HeaderName::from_static("x-two"), Counter::default());
        map.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(map.get("x-one").unwrap().0, 1);
        assert_eq!(map.get("x-two").unwrap().0, 1);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn request_relocate_calls_relocate_on_body() {
        let affinities = pinned_affinities(&[2]);
        let mut req = Request::new(Counter::default());
        req.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(req.body().0, 1);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn response_relocate_calls_relocate_on_body() {
        let affinities = pinned_affinities(&[2]);
        let mut resp = Response::new(Counter::default());
        resp.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(resp.body().0, 1);
    }
}
