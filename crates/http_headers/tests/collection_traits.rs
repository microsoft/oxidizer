// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Collection-trait and hashing integration coverage.

#![cfg(feature = "headers-all")]

use std::collections::{HashMap, HashSet};

use http_headers::headers::{CacheControlOwned, ContentTypeOwned, ETagOwned, LocationOwned, SetCookie, SetCookieOwned, UserAgentOwned};
use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{Field, FieldName, FieldValue};

#[derive(Default)]
struct TestSink(HashMap<FieldName, Vec<FieldValue>>);

impl FieldSource for TestSink {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        self.0.get(name).and_then(|values| FieldLines::from_slice(name, values))
    }
}

impl FieldSink for TestSink {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        if values.is_empty() {
            self.0.remove(name);
        } else {
            self.0.insert(name.clone(), values.into_iter().collect());
        }
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.0.entry(name.clone()).or_default().extend(values);
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        self.0.remove(name);
    }
}

#[test]
fn encoded_values_support_collection_iteration() {
    let mut encoded: EncodedValues = [FieldValue::from_static("first"), FieldValue::from_static("second")]
        .into_iter()
        .collect();

    assert_eq!((&encoded).into_iter().len(), 2);
    for value in &mut encoded {
        value.set_sensitivity(http_headers::FieldSensitivity::Sensitive);
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
        value.set_sensitivity(http_headers::FieldSensitivity::Sensitive);
    }
    let rebuilt = cookies
        .into_iter()
        .try_fold(SetCookieOwned::new(), |mut rebuilt, value| {
            rebuilt.push(value)?;
            Ok::<_, http_headers::DecodeError>(rebuilt)
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

    let mut table = TestSink::default();
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
        let mut map = http::HeaderMap::new();
        SetCookie::insert(&mut map, "b=2".parse::<SetCookieOwned>().expect("valid cookie")).expect("HTTP map accepts cookie");
        assert!(SetCookie::view(&map).expect("valid HTTP view").is_some());
        assert!(SetCookie::owned(&map).expect("valid HTTP value").is_some());
        map.insert(http::header::SET_COOKIE, http::HeaderValue::from_static(""));
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
        http_headers::headers::AcceptOwned,
        http_headers::headers::AcceptEncodingOwned,
        http_headers::headers::AcceptLanguageOwned,
        http_headers::headers::AcceptRangesOwned,
        http_headers::headers::AccessControlAllowCredentialsOwned,
        http_headers::headers::AccessControlAllowHeadersOwned,
        http_headers::headers::AccessControlAllowMethodsOwned,
        http_headers::headers::AccessControlAllowOriginOwned,
        http_headers::headers::AccessControlExposeHeadersOwned,
        http_headers::headers::AccessControlMaxAgeOwned,
        http_headers::headers::AccessControlRequestHeadersOwned,
        http_headers::headers::AccessControlRequestMethodOwned,
        http_headers::headers::AllowOwned,
        http_headers::headers::CacheControlOwned,
        http_headers::headers::ContentRangeOwned,
        http_headers::headers::ContentSecurityPolicyOwned,
        http_headers::headers::ContentTypeOwned,
        http_headers::headers::ETagOwned,
        http_headers::headers::HostOwned,
        http_headers::headers::IfMatchOwned,
        http_headers::headers::IfModifiedSinceOwned,
        http_headers::headers::IfNoneMatchOwned,
        http_headers::headers::IfRangeOwned,
        http_headers::headers::IfUnmodifiedSinceOwned,
        http_headers::headers::LastModifiedOwned,
        http_headers::headers::LocationOwned,
        http_headers::headers::RangeOwned,
        http_headers::headers::ReferrerPolicyOwned,
        http_headers::headers::SecWebSocketAcceptOwned,
        http_headers::headers::SecWebSocketExtensionsOwned,
        http_headers::headers::SecWebSocketKeyOwned,
        http_headers::headers::SecWebSocketProtocolOwned,
        http_headers::headers::SecWebSocketVersionOwned,
        http_headers::headers::ServerOwned,
        http_headers::headers::StrictTransportSecurityOwned,
        http_headers::headers::UserAgentOwned,
        http_headers::headers::VaryOwned,
        http_headers::headers::XContentTypeOptionsOwned,
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

    assert_display!(http_headers::headers::ContentRangeOwned, "bytes 0-1/2");
    assert_display!(http_headers::headers::HostOwned, "example.com");
    assert_display!(http_headers::headers::IfModifiedSinceOwned, "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_display!(http_headers::headers::IfUnmodifiedSinceOwned, "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_display!(http_headers::headers::LastModifiedOwned, "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_display!(http_headers::headers::RangeOwned, "bytes=0-1");
    assert_display!(http_headers::headers::SecWebSocketAcceptOwned, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    assert_display!(http_headers::headers::SecWebSocketKeyOwned, "dGhlIHNhbXBsZSBub25jZQ==");
    assert_display!(http_headers::headers::XContentTypeOptionsOwned, "nosniff");
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
