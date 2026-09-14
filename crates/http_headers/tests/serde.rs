// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serde round-trip and validation coverage for owned header data.

#![cfg(feature = "headers-all")]
#![cfg(feature = "serde")]
#![expect(clippy::assertions_on_result_states, reason = "rejection is the complete contract under test")]
#![expect(
    clippy::string_lit_as_bytes,
    reason = "string literals keep the exhaustive byte-oriented table readable"
)]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use http_headers::headers::*;
use http_headers::sink::EncodedValues;
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES, MAX_CUSTOM_FIELD_LINES, MAX_CUSTOM_LIST_ITEMS};
use http_headers::{Field, FieldName, FieldSensitivity, FieldValue};
use serde::de::value::{BorrowedBytesDeserializer, Error as ValueError};
use serde::de::{DeserializeOwned, DeserializeSeed, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, forward_to_deserialize_any};

struct Values(Vec<FieldValue>);

impl FieldSource for Values {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        FieldLines::from_slice(name, &self.0)
    }
}

fn assert_owned_round_trip<H>(lines: &[&[u8]])
where
    H: Field,
    H::Owned: Clone + DeserializeOwned + Serialize,
{
    let source = Values(lines.iter().map(|line| FieldValue::from_bytes(line).unwrap()).collect());
    let original = H::owned(&source).unwrap().unwrap();
    let json = serde_json::to_string(&original).unwrap();
    let decoded: H::Owned = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), serde_json::to_value(original).unwrap());
}

#[test]
fn every_owned_header_round_trips_through_validated_field_values() {
    macro_rules! cases {
        ($(($header:ty, $($line:expr),+)),+ $(,)?) => {
            $(assert_owned_round_trip::<$header>(&[$($line.as_bytes()),+]));+
        };
    }

    cases!(
        (CacheControl, "public, max-age=60"),
        (Accept, "text/html"),
        (AcceptEncoding, "gzip"),
        (AcceptLanguage, "en-US"),
        (Allow, "GET, POST"),
        (Host, "example.com:443"),
        (Server, "example/1.0"),
        (Vary, "accept-encoding"),
        (AcceptRanges, "bytes"),
        (ContentRange, "bytes 0-9/10"),
        (Range, "bytes=0-9"),
        (ETag, "\"tag\""),
        (Location, "/next"),
        (UserAgent, "example-client/1.0"),
        (ContentType, "text/plain; charset=utf-8"),
        (IfMatch, "\"tag\""),
        (IfNoneMatch, "*"),
        (IfModifiedSince, "Sun, 06 Nov 1994 08:49:37 GMT"),
        (IfUnmodifiedSince, "Sun, 06 Nov 1994 08:49:37 GMT"),
        (IfRange, "\"tag\""),
        (LastModified, "Sun, 06 Nov 1994 08:49:37 GMT"),
        (AccessControlAllowCredentials, "true"),
        (AccessControlAllowHeaders, "content-type, x-request-id"),
        (AccessControlAllowMethods, "GET, POST"),
        (AccessControlAllowOrigin, "https://example.com"),
        (AccessControlExposeHeaders, "etag, x-request-id"),
        (AccessControlMaxAge, "600"),
        (AccessControlRequestHeaders, "content-type"),
        (AccessControlRequestMethod, "POST"),
        (ContentSecurityPolicy, "default-src 'self'"),
        (ReferrerPolicy, "no-referrer"),
        (StrictTransportSecurity, "max-age=31536000; includeSubDomains"),
        (XContentTypeOptions, "nosniff"),
        (SecWebSocketAccept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        (SecWebSocketExtensions, "permessage-deflate"),
        (SecWebSocketKey, "dGhlIHNhbXBsZSBub25jZQ=="),
        (SecWebSocketProtocol, "chat"),
        (SecWebSocketVersion, "13"),
        (Authorization<Basic>, "Basic dXNlcjpwYXNz"),
        (Authorization<Bearer>, "Bearer token"),
        (ContentLength, "42"),
        (SetCookie, "session=abc; Path=/", "theme=dark; Path=/"),
    );
}

#[test]
fn core_types_preserve_names_bytes_sensitivity_and_line_boundaries() {
    let name = FieldName::try_from_bytes("X-Example").unwrap();
    let decoded_name: FieldName = serde_json::from_str(&serde_json::to_string(&name).unwrap()).unwrap();
    assert_eq!(decoded_name.as_str(), "x-example");

    let binary = FieldValue::from_bytes(b"text\x80")
        .unwrap()
        .with_sensitivity(FieldSensitivity::Sensitive);
    let decoded_binary: FieldValue = serde_json::from_str(&serde_json::to_string(&binary).unwrap()).unwrap();
    assert_eq!(decoded_binary.as_bytes(), b"text\x80");
    assert!(decoded_binary.is_sensitive());

    let values = EncodedValues::from_vec(vec![
        FieldValue::from_static("first"),
        FieldValue::from_static("second").with_sensitivity(FieldSensitivity::Sensitive),
    ]);
    let decoded_values: EncodedValues = serde_json::from_str(&serde_json::to_string(&values).unwrap()).unwrap();
    let decoded_values = decoded_values.into_iter().collect::<Vec<_>>();
    assert_eq!(decoded_values.len(), 2);
    assert_eq!(decoded_values[0].as_bytes(), b"first");
    assert_eq!(decoded_values[1].as_bytes(), b"second");
    assert!(decoded_values[1].is_sensitive());
}

#[cfg(feature = "http")]
#[test]
fn http_field_names_round_trip_through_serde() {
    let name = FieldName::from(http::HeaderName::from_lowercase(b"x\"y").unwrap());
    let decoded: FieldName = serde_json::from_str(&serde_json::to_string(&name).unwrap()).unwrap();
    assert_eq!(decoded, name);
}

#[test]
fn repeated_cache_and_range_lines_round_trip_without_collapsing() {
    for (name, values) in [
        (
            &FieldName::CacheControl,
            vec![FieldValue::from_static("public"), FieldValue::from_static("max-age=60")],
        ),
        (
            &FieldName::AcceptRanges,
            vec![FieldValue::from_static("bytes"), FieldValue::from_static("items")],
        ),
    ] {
        let expected = serde_json::to_value(&values).unwrap();
        let source = Values(values);
        if name == &FieldName::CacheControl {
            let original = CacheControl::owned(&source).unwrap().unwrap();
            assert_eq!(serde_json::to_value(&original).unwrap(), expected);
            let decoded: CacheControlOwned = serde_json::from_value(expected).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), serde_json::to_value(original).unwrap());
        } else {
            let original = AcceptRanges::owned(&source).unwrap().unwrap();
            assert_eq!(serde_json::to_value(&original).unwrap(), expected);
            let decoded: AcceptRangesOwned = serde_json::from_value(expected).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), serde_json::to_value(original).unwrap());
        }
    }

    let none = AcceptRangesOwned::none();
    let expected = serde_json::to_value([FieldValue::from_static("none")]).unwrap();
    assert_eq!(serde_json::to_value(none).unwrap(), expected);
}

#[test]
fn relaxed_owned_values_round_trip_through_serde() {
    let source = Values(vec![FieldValue::from_static("/a\\b")]);
    let original = Location::owned_with(&source, http_headers::DecodeMode::Relaxed).unwrap().unwrap();
    let decoded: LocationOwned = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(decoded.as_bytes(), original.as_bytes());
}

#[test]
fn typed_deserialization_reuses_header_validation() {
    let invalid_value = r#"[{"bytes":[10],"sensitivity":"NonSensitive"}]"#;
    assert!(serde_json::from_str::<UserAgentOwned>(invalid_value).is_err());

    let multiple_values = r#"[{"bytes":[116,101,120,116,47,112,108,97,105,110],"sensitivity":"NonSensitive"},{"bytes":[116,101,120,116,47,104,116,109,108],"sensitivity":"NonSensitive"}]"#;
    assert!(serde_json::from_str::<ContentTypeOwned>(multiple_values).is_err());

    let invalid_name = serde_json::to_string("bad\nname").unwrap();
    assert!(serde_json::from_str::<FieldName>(&invalid_name).is_err());

    let extension_method = AccessControlRequestMethodOwned::try_from("CUSTOM").unwrap();
    let extension_json = serde_json::to_string(&extension_method).unwrap();
    let decoded_extension: AccessControlRequestMethodOwned = serde_json::from_str(&extension_json).unwrap();
    assert_eq!(decoded_extension.method().unwrap().as_str(), "CUSTOM");
}

#[test]
fn empty_set_cookie_round_trips_without_becoming_absent() {
    let json = serde_json::to_string(&SetCookieOwned::new()).unwrap();
    let decoded: SetCookieOwned = serde_json::from_str(&json).unwrap();
    assert!(decoded.is_empty());
}

#[test]
fn fixed_payloads_pin_the_successful_serde_representation() {
    const NAME: &str = r#""x-example""#;
    const SENSITIVE_BINARY: &str = r#"{"bytes":[116,101,120,116,128],"sensitivity":"Sensitive"}"#;
    const ENCODED_VALUES: &str =
        r#"[{"bytes":[102,105,114,115,116],"sensitivity":"NonSensitive"},{"bytes":[115,101,99,111,110,100],"sensitivity":"Sensitive"}]"#;
    const USER_AGENT: &str =
        r#"[{"bytes":[101,120,97,109,112,108,101,45,99,108,105,101,110,116,47,49,46,48],"sensitivity":"NonSensitive"}]"#;
    const ACCEPT: &str = r#"[{"bytes":[116,101,120,116,47,104,116,109,108],"sensitivity":"NonSensitive"},{"bytes":[97,112,112,108,105,99,97,116,105,111,110,47,106,115,111,110],"sensitivity":"NonSensitive"}]"#;
    const SET_COOKIE: &str = r#"[{"bytes":[115,101,115,115,105,111,110,61,97,98,99],"sensitivity":"Sensitive"},{"bytes":[116,104,101,109,101,61,100,97,114,107],"sensitivity":"Sensitive"}]"#;

    let name: FieldName = serde_json::from_str(NAME).unwrap();
    assert_eq!(name.as_str(), "x-example");
    assert_eq!(serde_json::to_string(&name).unwrap(), NAME);

    let binary: FieldValue = serde_json::from_str(SENSITIVE_BINARY).unwrap();
    assert_eq!(binary.as_bytes(), b"text\x80");
    assert!(binary.is_sensitive());
    assert_eq!(serde_json::to_string(&binary).unwrap(), SENSITIVE_BINARY);

    let values: EncodedValues = serde_json::from_str(ENCODED_VALUES).unwrap();
    let lines = values.iter().collect::<Vec<_>>();
    assert_eq!(lines[0].as_bytes(), b"first");
    assert_eq!(lines[1].as_bytes(), b"second");
    assert!(!lines[0].is_sensitive());
    assert!(lines[1].is_sensitive());
    assert_eq!(serde_json::to_string(&values).unwrap(), ENCODED_VALUES);

    let user_agent: UserAgentOwned = serde_json::from_str(USER_AGENT).unwrap();
    assert_eq!(user_agent.as_bytes(), b"example-client/1.0");
    assert_eq!(serde_json::to_string(&user_agent).unwrap(), USER_AGENT);

    let accept: AcceptOwned = serde_json::from_str(ACCEPT).unwrap();
    assert_eq!(
        accept.values().map(http_headers::FieldValueRef::as_bytes).collect::<Vec<_>>(),
        [b"text/html".as_slice(), b"application/json".as_slice()]
    );
    assert_eq!(serde_json::to_string(&accept).unwrap(), ACCEPT);

    let cookies: SetCookieOwned = serde_json::from_str(SET_COOKIE).unwrap();
    assert_eq!(
        cookies.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(),
        [b"session=abc".as_slice(), b"theme=dark".as_slice()]
    );
    assert!(cookies.iter().all(FieldValue::is_sensitive));
    assert_eq!(serde_json::to_string(&cookies).unwrap(), SET_COOKIE);
}

struct HostileBytesDeserializer {
    remaining: usize,
    reported: usize,
}

struct HostileBytesAccess {
    remaining: usize,
    reported: usize,
}

impl<'de> SeqAccess<'de> for HostileBytesAccess {
    type Error = ValueError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(b'a'.into_deserializer()).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.reported)
    }
}

impl<'de> Deserializer<'de> for HostileBytesDeserializer {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(HostileBytesAccess {
            remaining: self.remaining,
            reported: self.reported,
        })
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct tuple tuple_struct map struct enum identifier
        ignored_any
    }
}

struct HostileFieldValueDeserializer {
    byte_count: usize,
    byte_hint: usize,
}

struct HostileFieldValueMap {
    state: u8,
    byte_count: usize,
    byte_hint: usize,
}

impl<'de> MapAccess<'de> for HostileFieldValueMap {
    type Error = ValueError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        let key = match self.state {
            0 => "ignored",
            1 => "bytes",
            2 => "sensitivity",
            _ => return Ok(None),
        };
        self.state += 1;
        seed.deserialize(BorrowedBytesDeserializer::new(key.as_bytes())).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        match self.state {
            1 => seed.deserialize(0_u8.into_deserializer()),
            2 => seed.deserialize(HostileBytesDeserializer {
                remaining: self.byte_count,
                reported: self.byte_hint,
            }),
            3 => seed.deserialize("NonSensitive".into_deserializer()),
            _ => Err(serde::de::Error::custom("value requested without a key")),
        }
    }
}

impl<'de> Deserializer<'de> for HostileFieldValueDeserializer {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(HostileFieldValueMap {
            state: 0,
            byte_count: self.byte_count,
            byte_hint: self.byte_hint,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_map(visitor)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct enum identifier ignored_any
    }
}

struct HostileValuesDeserializer {
    remaining: usize,
    reported: usize,
    byte_count: usize,
    byte_hint: usize,
}

impl<'de> SeqAccess<'de> for HostileValuesDeserializer {
    type Error = ValueError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(HostileFieldValueDeserializer {
            byte_count: self.byte_count,
            byte_hint: self.byte_hint,
        })
        .map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.reported)
    }
}

impl<'de> Deserializer<'de> for HostileValuesDeserializer {
    type Error = ValueError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(self)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct tuple tuple_struct map struct enum identifier
        ignored_any
    }
}

#[test]
fn hostile_collection_hints_cannot_drive_unbounded_deserialization_allocations() {
    let valid = EncodedValues::deserialize(HostileValuesDeserializer {
        remaining: 1,
        reported: usize::MAX,
        byte_count: 1,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(valid.iter().next().unwrap().as_bytes(), b"a");

    let byte_boundary = FieldValue::deserialize(HostileFieldValueDeserializer {
        byte_count: MAX_CUSTOM_FIELD_BYTES,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(byte_boundary.as_bytes().len(), MAX_CUSTOM_FIELD_BYTES);

    let oversized_bytes = FieldValue::deserialize(HostileFieldValueDeserializer {
        byte_count: MAX_CUSTOM_FIELD_BYTES + 1,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(oversized_bytes.as_bytes().len(), MAX_CUSTOM_FIELD_BYTES + 1);

    let line_boundary = EncodedValues::deserialize(HostileValuesDeserializer {
        remaining: MAX_CUSTOM_FIELD_LINES,
        reported: usize::MAX,
        byte_count: 0,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(line_boundary.len(), MAX_CUSTOM_FIELD_LINES);

    let oversized_lines = EncodedValues::deserialize(HostileValuesDeserializer {
        remaining: MAX_CUSTOM_FIELD_LINES + 1,
        reported: usize::MAX,
        byte_count: 0,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(oversized_lines.len(), MAX_CUSTOM_FIELD_LINES + 1);

    let oversized_typed_bytes = UserAgentOwned::deserialize(HostileValuesDeserializer {
        remaining: 1,
        reported: usize::MAX,
        byte_count: MAX_CUSTOM_FIELD_BYTES + 1,
        byte_hint: usize::MAX,
    });
    assert!(oversized_typed_bytes.is_err());

    let line_boundary = SetCookieOwned::deserialize(HostileValuesDeserializer {
        remaining: MAX_CUSTOM_FIELD_LINES,
        reported: usize::MAX,
        byte_count: 1,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(line_boundary.len(), MAX_CUSTOM_FIELD_LINES);

    let oversized_typed_lines = SetCookieOwned::deserialize(HostileValuesDeserializer {
        remaining: MAX_CUSTOM_FIELD_LINES + 1,
        reported: usize::MAX,
        byte_count: 1,
        byte_hint: usize::MAX,
    });
    assert!(oversized_typed_lines.is_err());
}

#[test]
fn typed_serde_enforces_list_item_budgets_at_the_collection_boundary() {
    let boundary = std::iter::repeat_n("*/*", MAX_CUSTOM_LIST_ITEMS).collect::<Vec<_>>().join(",");
    let boundary = EncodedValues::single(FieldValue::try_from(boundary).unwrap());
    let json = serde_json::to_string(&boundary).unwrap();
    assert!(serde_json::from_str::<AcceptOwned>(&json).is_ok());

    let over_limit = std::iter::repeat_n("*/*", MAX_CUSTOM_LIST_ITEMS + 1).collect::<Vec<_>>().join(",");
    let over_limit = EncodedValues::single(FieldValue::try_from(over_limit).unwrap());
    let json = serde_json::to_string(&over_limit).unwrap();
    assert!(serde_json::from_str::<AcceptOwned>(&json).is_err());
}

#[test]
fn field_value_deserialization_covers_sequence_metadata_and_diagnostics() {
    let sequence: FieldValue = serde_json::from_str(r#"[[97],"NonSensitive"]"#).unwrap();
    assert_eq!(sequence.as_bytes(), b"a");

    let with_unknown: FieldValue = serde_json::from_str(r#"{"ignored":0,"bytes":[97],"sensitivity":"NonSensitive"}"#).unwrap();
    assert_eq!(with_unknown.as_bytes(), b"a");

    for invalid in [
        r#"{"bytes":[97],"bytes":[98],"sensitivity":"NonSensitive"}"#,
        r#"{"bytes":[97],"sensitivity":"NonSensitive","sensitivity":"Sensitive"}"#,
        r#"{"sensitivity":"NonSensitive"}"#,
        r#"{"bytes":[97]}"#,
        "null",
    ] {
        assert!(serde_json::from_str::<FieldValue>(invalid).is_err(), "{invalid}");
    }
    assert!(serde_json::from_str::<FieldValue>(r#"{"bytes":"not bytes","sensitivity":"NonSensitive"}"#).is_err());
    assert!(serde_json::from_str::<EncodedValues>("null").is_err());

    let boundary = UserAgentOwned::deserialize(HostileValuesDeserializer {
        remaining: 1,
        reported: usize::MAX,
        byte_count: MAX_CUSTOM_FIELD_BYTES,
        byte_hint: usize::MAX,
    })
    .unwrap();
    assert_eq!(boundary.as_bytes().len(), MAX_CUSTOM_FIELD_BYTES);
}
