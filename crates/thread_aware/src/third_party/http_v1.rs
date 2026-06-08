// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`http`](::http_v1) (1.x) types.
//!
//! Enable with the `http_v1` Cargo feature.
//!
//! All inert value types in `http` (`StatusCode`, `Method`, `Version`,
//! `HeaderName`, `HeaderValue`) get a no-op `relocate`. The `HeaderMap`
//! impl is provided for `HeaderMap<HeaderValue>` only (the default
//! parameterisation produced by the `http` crate) and is also a no-op,
//! since `HeaderValue::relocate` is itself no-op — iterating would be
//! pure waste.
//!
//! `Request<T>::relocate` and `Response<T>::relocate` forward to the body
//! only. Their headers are `HeaderMap<HeaderValue>`, which is inert by the
//! impl above, so iterating header values would also be wasted work.
//!
//! Note: `http::Extensions` (carried by `Request<T>` and `Response<T>`) holds
//! arbitrary `Any` values whose concrete types are erased at runtime, so this
//! impl cannot relocate them. Callers that stash thread-affine state in
//! extensions must relocate it explicitly.

use ::http_v1::header::{HeaderMap, HeaderName, HeaderValue};
use ::http_v1::{Method, Request, Response, StatusCode, Version};

use crate::ThreadAware;
use crate::affinity::Affinity;

impl_noop_thread_aware!(StatusCode, Version, Method, HeaderName, HeaderValue, HeaderMap<HeaderValue>);

impl<T: ThreadAware> ThreadAware for Request<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.body_mut().relocate(source, destination);
    }
}

impl<T: ThreadAware> ThreadAware for Response<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.body_mut().relocate(source, destination);
    }
}

#[cfg(test)]
mod tests {
    use ::http_v1::header::{HeaderMap, HeaderName, HeaderValue};
    use ::http_v1::{Method, Request, Response, StatusCode, Version};
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
