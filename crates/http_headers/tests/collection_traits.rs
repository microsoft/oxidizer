// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Collection-trait and hashing integration coverage.

#![cfg(feature = "headers-all")]

use std::collections::{HashMap, HashSet};

#[cfg(feature = "http")]
use http::{HeaderMap, HeaderValue, header};
use http_headers::headers::{
    self, CacheControlOwned, ContentTypeOwned, ETagOwned, LocationOwned, SetCookie, SetCookieOwned, UserAgentOwned,
};
use http_headers::sink::{EncodedValues, FieldSink};
use http_headers::{DecodeError, Field, FieldSensitivity, FieldValue};

use self::common::TestMap;

mod common;

#[test]
fn encoded_values_support_collection_iteration() {
    let mut encoded: EncodedValues = [FieldValue::from_static("first"), FieldValue::from_static("second")]
        .into_iter()
        .collect();

    assert_eq!((&encoded).into_iter().len(), 2);
    for value in &mut encoded {
        value.set_sensitivity(FieldSensitivity::Sensitive);
    }
    assert!(encoded.iter().all(FieldValue::is_sensitive));
    assert_eq!(encoded.into_iter().len(), 2);
}

#[test]
fn set_cookie_supports_safe_collection_iteration() {
    let mut cookies = SetCookieOwned::new();
    for value in ["a=1", "b=2"] {
        cookies.push_str(value).expect("valid cookie");
    }

    assert_eq!((&cookies).into_iter().len(), 2);
    for value in &mut cookies {
        value.set_sensitivity(FieldSensitivity::Sensitive);
    }
    let rebuilt = cookies
        .into_iter()
        .try_fold(SetCookieOwned::new(), |mut rebuilt, value| {
            rebuilt.push(value)?;
            Ok::<_, DecodeError>(rebuilt)
        })
        .expect("stored values preserve the Set-Cookie invariant");
    assert_eq!(rebuilt.len(), 2);
}

#[test]
fn set_cookie_public_helpers_cover_empty_invalid_and_view_states() {
    let mut cookies = SetCookieOwned::default();
    assert!(cookies.is_empty());
    assert!(cookies.iter_mut().next().is_none());
    assert!(cookies.push_str("\n").is_err());

    let parsed = "a=1".parse::<SetCookieOwned>().expect("valid cookie");
    assert!(!parsed.is_empty());
    assert!(format!("{parsed:?}").contains("value_count"));

    let mut table = TestMap::default();
    SetCookie::insert(&mut table, parsed).expect("table accepts cookie");
    let view = SetCookie::view(&table).expect("valid cookie view").expect("cookie is present");
    assert!(!view.is_empty());

    table
        .set_values(SetCookie::name(), EncodedValues::single(FieldValue::from_static("")))
        .expect("table accepts raw empty field value");
    SetCookie::view(&table).expect_err("empty cookie is invalid");
    SetCookie::owned(&table).expect_err("empty cookie is invalid");

    #[cfg(feature = "http")]
    {
        let mut map = HeaderMap::new();
        SetCookie::insert(&mut map, "b=2".parse::<SetCookieOwned>().expect("valid cookie")).expect("HTTP map accepts cookie");
        assert!(SetCookie::view(&map).expect("valid HTTP view").is_some());
        assert!(SetCookie::owned(&map).expect("valid HTTP value").is_some());
        map.insert(header::SET_COOKIE, HeaderValue::from_static(""));
        SetCookie::view(&map).expect_err("empty cookie is invalid");
        SetCookie::owned(&map).expect_err("empty cookie is invalid");
    }
}

#[test]
fn shared_from_str_error_mapping_covers_foundational_headers() {
    "\n".parse::<CacheControlOwned>().expect_err("invalid field");
    "\n".parse::<ContentTypeOwned>().expect_err("invalid field");
    "\n".parse::<ETagOwned>().expect_err("invalid field");
    "\n".parse::<LocationOwned>().expect_err("invalid field");
    "\n".parse::<UserAgentOwned>().expect_err("invalid field");
}

#[test]
fn shared_from_str_error_mapping_covers_every_generated_impl() {
    macro_rules! assert_invalid {
        ($($owned:path),+ $(,)?) => {
            $("\n".parse::<$owned>().expect_err("invalid field");)+
        };
    }

    assert_invalid!(
        headers::AcceptOwned,
        headers::AcceptEncodingOwned,
        headers::AcceptLanguageOwned,
        headers::AcceptRangesOwned,
        headers::AccessControlAllowCredentialsOwned,
        headers::AccessControlAllowHeadersOwned,
        headers::AccessControlAllowMethodsOwned,
        headers::AccessControlAllowOriginOwned,
        headers::AccessControlExposeHeadersOwned,
        headers::AccessControlMaxAgeOwned,
        headers::AccessControlRequestHeadersOwned,
        headers::AccessControlRequestMethodOwned,
        headers::AllowOwned,
        headers::CacheControlOwned,
        headers::ContentRangeOwned,
        headers::ContentSecurityPolicyOwned,
        headers::ContentTypeOwned,
        headers::ETagOwned,
        headers::HostOwned,
        headers::IfMatchOwned,
        headers::IfModifiedSinceOwned,
        headers::IfNoneMatchOwned,
        headers::IfRangeOwned,
        headers::IfUnmodifiedSinceOwned,
        headers::LastModifiedOwned,
        headers::LocationOwned,
        headers::RangeOwned,
        headers::ReferrerPolicyOwned,
        headers::SecWebSocketAcceptOwned,
        headers::SecWebSocketExtensionsOwned,
        headers::SecWebSocketKeyOwned,
        headers::SecWebSocketProtocolOwned,
        headers::SecWebSocketVersionOwned,
        headers::ServerOwned,
        headers::StrictTransportSecurityOwned,
        headers::UserAgentOwned,
        headers::VaryOwned,
        headers::XContentTypeOptionsOwned,
    );
}

#[test]
fn shared_ascii_display_covers_every_generated_impl() {
    macro_rules! assert_display {
        ($owned:path, $wire:literal) => {{
            let value = $wire.parse::<$owned>().expect("valid display value");
            assert_eq!(value.to_string(), $wire);
        }};
    }

    assert_display!(headers::ContentRangeOwned, "bytes 0-1/2");
    assert_display!(headers::HostOwned, "example.com");
    assert_display!(headers::IfModifiedSinceOwned, "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_display!(headers::IfUnmodifiedSinceOwned, "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_display!(headers::LastModifiedOwned, "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_display!(headers::RangeOwned, "bytes=0-1");
    assert_display!(headers::SecWebSocketAcceptOwned, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    assert_display!(headers::SecWebSocketKeyOwned, "dGhlIHNhbXBsZSBub25jZQ==");
    assert_display!(headers::XContentTypeOptionsOwned, "nosniff");
}

#[test]
fn public_equal_types_are_hashable() {
    let mut tags = HashSet::new();
    tags.insert(ETagOwned::strong("revision").expect("valid entity tag"));
    assert_eq!(tags.len(), 1);

    let mut media = HashMap::new();
    media.insert(ContentTypeOwned::try_from("application/json").expect("valid media type"), "json");
    assert_eq!(media.len(), 1);
}
