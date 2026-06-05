// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ThreadAware` impls for [`http`] (1.x) types.
//!
//! Enable with the `http_v1` Cargo feature.
//!
//! Inert value types (`StatusCode`, `Method`, `Version`, `HeaderName`,
//! `HeaderValue`) get a no-op `relocate`. Container types (`HeaderMap<T>`,
//! `Request<T>`, `Response<T>`) propagate `relocate` to every element they own;
//! `Request<T>` and `Response<T>` relocate both their header values and body,
//! mirroring how this crate handles `Vec<T>` and `Box<T>`.

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
    #[cfg(feature = "threads")]
    use crate::affinity::pinned_affinities;

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
    fn request_relocate_propagates_to_body() {
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
}
