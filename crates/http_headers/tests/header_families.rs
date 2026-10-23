// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public header-family behavior moved out of source modules.

#![cfg(feature = "headers-all")]
//!
//! These tests cover the `http` adapter, so the file is compiled only when the
//! `http` feature is enabled.

#![cfg(feature = "http")]

use http::{HeaderMap, HeaderValue};
use http_headers::headers::{ContentLength, ContentLengthOwned, ETagOwned, ETagView, SetCookie, SetCookieOwned};
use http_headers::{DecodeErrorKind, FieldValue, FieldValueRef};

const _: [(); size_of::<ETagView<'_>>()] = [(); size_of::<FieldValueRef<'_>>()];

#[test]
fn entity_tag_comparison_and_construction() {
    let strong = ETagOwned::strong("revision-42").expect("valid strong tag");
    let same = ETagOwned::strong("revision-42").expect("valid strong tag");
    let weak = ETagOwned::weak("revision-42").expect("valid weak tag");
    assert_eq!(strong.strong_eq(&same), Ok(true));
    assert_eq!(strong.strong_eq(&weak), Ok(false));
    assert_eq!(strong.weak_eq(&weak), Ok(true));
    assert_eq!(weak.opaque_tag(), Ok(b"revision-42".as_slice()));
    let wire = ETagOwned::weak("abc").expect("valid weak tag").into_field_value();
    assert_eq!(wire, "W/\"abc\"");
    assert_eq!(
        ETagOwned::strong("bad\"tag").expect_err("quote must be rejected").kind(),
        DecodeErrorKind::InvalidSyntax
    );
}

#[test]
fn content_length_accepts_matching_and_rejects_conflicting_values() {
    let mut map = HeaderMap::new();
    map.append("content-length", HeaderValue::from_static("42"));
    map.append("content-length", HeaderValue::from_static("42, 42"));
    assert_eq!(ContentLength::view(&map), Ok(Some(ContentLengthOwned::new(42))));

    map.clear();
    map.append("content-length", HeaderValue::from_static("1"));
    map.append("content-length", HeaderValue::from_static("2"));
    assert_eq!(
        ContentLength::view(&map).expect_err("conflicting lengths must fail").kind(),
        DecodeErrorKind::InvalidSyntax
    );

    map.clear();
    map.insert("content-length", HeaderValue::from_static("18446744073709551616"));
    assert_eq!(
        ContentLength::view(&map).expect_err("overflow must fail").kind(),
        DecodeErrorKind::InvalidNumber
    );
}

#[test]
fn set_cookie_preserves_boundaries_sensitivity_and_opaque_attributes() {
    let mut map = HeaderMap::new();
    map.append("set-cookie", HeaderValue::from_static("a=1; Path=/"));
    map.append("set-cookie", HeaderValue::from_static("b=2; HttpOnly"));
    let view = SetCookie::view(&map).expect("valid cookies").expect("cookies present");
    assert_eq!(
        view.iter().map(FieldValueRef::as_bytes).collect::<Vec<_>>(),
        [b"a=1; Path=/".as_slice(), b"b=2; HttpOnly".as_slice()]
    );

    let owned = SetCookie::owned(&map).expect("valid cookies").expect("cookies present");
    assert_eq!(owned.len(), 2);
    assert!(owned.iter().all(FieldValue::is_sensitive));

    let mut inserted = SetCookieOwned::new();
    inserted.push_str("a=1").expect("valid cookie");
    inserted.push_str("b=2").expect("valid cookie");
    let mut inserted_map = HeaderMap::new();
    SetCookie::insert(&mut inserted_map, inserted).expect("map has capacity");
    assert_eq!(inserted_map.get_all("set-cookie").iter().count(), 2);

    let mut secret_map = HeaderMap::new();
    secret_map.insert("set-cookie", HeaderValue::from_static("session=super-secret"));
    let debug = format!("{:?}", SetCookie::view(&secret_map).expect("valid cookie").expect("cookie present"));
    assert!(!debug.contains("super-secret"));
    assert!(debug.contains("value_count"));

    let mut cookies = SetCookieOwned::new();
    cookies
        .push_str("session=abc; Max-Age=0")
        .expect("cookie grammar belongs to the caller");
    assert_eq!(
        cookies.iter().next().map(FieldValue::as_bytes),
        Some(b"session=abc; Max-Age=0".as_slice())
    );
}
