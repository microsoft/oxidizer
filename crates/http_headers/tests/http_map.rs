// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `http::HeaderMap` integration tests.

#![cfg(feature = "http")]

use std::sync::LazyLock;

use http::{HeaderMap, HeaderValue};
use http_headers::headers::{Allow, SetCookie, SetCookieOwned, UserAgent};
use http_headers::sink::{EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSink as _, FieldSinkExt, FieldValueWriter, InsertError};
use http_headers::source::FieldSource;
use http_headers::{DecodeError, Field, FieldName, FieldSensitivity, FieldValue, FieldValueRef, SingleValueField};

static TRACE_ID: LazyLock<FieldName> = LazyLock::new(|| FieldName::from_static("x-trace-id"));

struct TraceId(FieldValue);

struct BytesEncoder {
    expected: usize,
    bytes: &'static [u8],
    sensitive: bool,
}

impl FieldEncoder for BytesEncoder {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        let sensitivity = if self.sensitive {
            http_headers::FieldSensitivity::Sensitive
        } else {
            http_headers::FieldSensitivity::NonSensitive
        };
        let mut writer = output.begin_value(self.expected, sensitivity)?;
        writer.write_bytes(self.bytes)?;
        writer.finish()
    }
}

#[derive(Clone, Copy)]
#[expect(
    dead_code,
    reason = "the field proves the view carries the borrowed ref; decode_owned uses the owned value"
)]
struct TraceIdView<'a>(FieldValueRef<'a>);

impl SingleValueField for TraceId {
    type View<'a> = TraceIdView<'a>;
    type Owned = Self;

    fn name() -> &'static FieldName {
        &TRACE_ID
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        Ok(TraceIdView(value))
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        Ok(Self(value))
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.0
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.0
    }
}

#[test]
fn known_and_custom_names_reach_the_same_storage() {
    let mut map = HeaderMap::new();
    map.insert(http::header::USER_AGENT, HeaderValue::from_static("client/1"));
    map.insert("x-trace-id", HeaderValue::from_static("abc"));

    assert!(map.contains(&FieldName::UserAgent));
    assert!(map.contains(&TRACE_ID));
    assert_eq!(
        FieldSource::lines(&map, <TraceId as Field>::name())
            .expect("trace ID present")
            .len(),
        1
    );
    assert_eq!(
        FieldSource::lines(&map, <UserAgent as Field>::name())
            .expect("user agent present")
            .len(),
        1
    );
    assert_eq!(
        UserAgent::view(&map)
            .expect("valid user agent")
            .expect("user agent present")
            .as_bytes(),
        b"client/1"
    );
}

#[test]
fn setting_empty_values_removes_the_header() {
    let mut map = HeaderMap::new();
    map.insert(http::header::USER_AGENT, HeaderValue::from_static("client/1"));
    map.set_values(<UserAgent as Field>::name(), EncodedValues::new())
        .expect("removal always fits");

    assert!(!map.contains(&FieldName::UserAgent));
}

#[test]
fn repeated_field_lines_survive_a_round_trip() {
    let mut map = HeaderMap::new();
    let mut encoded = EncodedValues::new();
    encoded.push(FieldValue::from_static("a=1"));
    encoded.push(FieldValue::from_static("b=2"));
    map.set_values(<SetCookie as Field>::name(), encoded)
        .expect("an empty map has capacity");

    assert_eq!(map.get_all(http::header::SET_COOKIE).iter().count(), 2);
    assert_eq!(SetCookie::view(&map).expect("valid cookies").expect("cookies present").len(), 2);
}

fn assert_sensitive_cookie_values(map: &HeaderMap, expected: &[&[u8]]) {
    let values = map.get_all(http::header::SET_COOKIE).iter().collect::<Vec<_>>();
    assert_eq!(values.iter().map(|value| value.as_bytes()).collect::<Vec<_>>(), expected);
    for value in values {
        assert!(value.is_sensitive());
        let debug = format!("{value:?}");
        assert!(
            !debug
                .as_bytes()
                .windows(value.as_bytes().len())
                .any(|bytes| bytes == value.as_bytes())
        );
    }
}

#[test]
fn typed_cookie_replacement_restores_cleared_sensitivity() {
    let mut map = HeaderMap::new();
    map.append(http::header::SET_COOKIE, HeaderValue::from_static("stale=1"));

    let mut replacement = SetCookieOwned::new();
    replacement.push_str("replacement-secret=first").expect("valid cookie");
    replacement.push_str("replacement-secret=second").expect("valid cookie");
    replacement
        .iter_mut()
        .for_each(|value| value.set_sensitivity(FieldSensitivity::NonSensitive));

    SetCookie::insert(&mut map, replacement).expect("the map has capacity");

    assert_sensitive_cookie_values(
        &map,
        &[b"replacement-secret=first".as_slice(), b"replacement-secret=second".as_slice()],
    );
}

#[test]
fn fluent_cookie_append_preserves_lines_and_restores_cleared_sensitivity() {
    let mut map = HeaderMap::new();
    let mut first = SetCookieOwned::new();
    first.push_str("a=1").expect("valid cookie");
    first.push_str("b=2").expect("valid cookie");
    first
        .iter_mut()
        .for_each(|value| value.set_sensitivity(FieldSensitivity::NonSensitive));
    let mut second = SetCookieOwned::new();
    second.push_str("c=3").expect("valid cookie");
    second.push_str("d=4").expect("valid cookie");
    second
        .iter_mut()
        .for_each(|value| value.set_sensitivity(FieldSensitivity::NonSensitive));

    map.append_set_cookie(first)
        .expect("an empty map has capacity")
        .append_set_cookie(second)
        .expect("the map has capacity");

    assert_sensitive_cookie_values(&map, &[b"a=1".as_slice(), b"b=2".as_slice(), b"c=3".as_slice(), b"d=4".as_slice()]);
}

#[test]
fn deferred_map_encoding_covers_numeric_writer_and_custom_removal() {
    let mut map = HeaderMap::new();
    map.set_content_length(42)
        .expect("numeric field fits")
        .set_access_control_max_age(600)
        .expect("numeric field fits");
    assert_eq!(map[http::header::CONTENT_LENGTH], "42");

    map.set_encoded(
        &TRACE_ID,
        BytesEncoder {
            expected: 3,
            bytes: b"abc",
            sensitive: true,
        },
    )
    .expect("custom value fits");
    assert!(map[TRACE_ID.as_str()].is_sensitive());
    map.remove_values(&TRACE_ID);
    assert!(!map.contains_key(TRACE_ID.as_str()));
}

#[test]
fn deferred_map_rejects_bad_encoders_without_replacement() {
    let mut map = HeaderMap::new();
    map.insert(http::header::USER_AGENT, HeaderValue::from_static("original"));

    for encoder in [
        BytesEncoder {
            expected: 2,
            bytes: b"x",
            sensitive: false,
        },
        BytesEncoder {
            expected: 1,
            bytes: b"\n",
            sensitive: false,
        },
        BytesEncoder {
            expected: 0,
            bytes: b"x",
            sensitive: false,
        },
        BytesEncoder {
            expected: usize::MAX,
            bytes: b"x",
            sensitive: false,
        },
    ] {
        assert_eq!(map.set_encoded(&FieldName::UserAgent, encoder), Err(InsertError));
        assert_eq!(map[http::header::USER_AGENT], "original");
    }
}

#[test]
fn validated_map_uses_optimized_token_list_decoders() {
    let mut map = HeaderMap::new();
    map.insert(http::header::ALLOW, HeaderValue::from_static("GET"));

    assert!(Allow::view(&map).expect("valid borrowed list").is_some());
    assert!(Allow::owned(&map).expect("valid owned list").is_some());
    assert!(
        Allow::owned_with(&map, http_headers::DecodeMode::Relaxed)
            .expect("valid relaxed owned list")
            .is_some()
    );

    map.insert(http::header::ALLOW, HeaderValue::from_static("GET, bad token"));
    Allow::owned(&map).expect_err("a token-list member cannot contain a space");
}
