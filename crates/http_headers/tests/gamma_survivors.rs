// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Header parser, writer, and value-contract regressions for surviving mutations.

#![cfg(feature = "headers-all")]

use std::cmp::Ordering;
use std::fmt::{self, Write as _};
use std::hash::{Hash, Hasher};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, UNIX_EPOCH};

use base64::Engine as _;
use http_headers::headers::*;
use http_headers::sink::{
    EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSink, FieldSinkExt, FieldValueWriter, InsertError, InsertErrorKind,
};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldSensitivity, FieldValue, FieldValueRef};

use self::common::TestMap;

mod common;

fn source<H: Field>(values: &[&str]) -> TestMap {
    let mut result = TestMap::default();
    result.0.insert(
        H::name().clone(),
        values
            .iter()
            .map(|value| FieldValue::from_str(value).expect("test source fixtures must contain valid field-value bytes"))
            .collect(),
    );
    result
}

fn accepts<H: Field>(values: &[&str]) {
    let source = source::<H>(values);
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert_eq!(H::view_with(&source, mode).map(|value| value.is_some()), Ok(true), "{values:?}");
        assert_eq!(H::owned_with(&source, mode).map(|value| value.is_some()), Ok(true), "{values:?}");
    }
}

fn rejects<H: Field>(values: &[&str]) {
    let source = source::<H>(values);
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert!(H::view_with(&source, mode).err().is_some(), "{values:?}");
        assert!(H::owned_with(&source, mode).err().is_some(), "{values:?}");
    }
}

#[derive(Default, Debug, Eq, PartialEq)]
struct HashWrites(Vec<u8>);

impl Hasher for HashWrites {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn finish(&self) -> u64 {
        0
    }
}

fn hash_writes(value: impl Hash) -> HashWrites {
    let mut writes = HashWrites::default();
    value.hash(&mut writes);
    writes
}

#[test]
fn gamma_survivor_names_and_values_have_distinct_equality_order_and_hashes() {
    for name in FieldName::ALL_KNOWN {
        let parsed = FieldName::try_from_bytes(name.as_bytes()).unwrap();
        assert_eq!(parsed, *name);
        assert_eq!(parsed.index(), name.index());
        assert_eq!(
            FieldName::try_from_bytes(name.as_str().to_ascii_uppercase().as_bytes()).unwrap(),
            *name
        );
        assert_eq!(name.as_str(), name.to_string());
    }
    for length in [1, 8, 63, 64, 65, 127, 128, 129, 255] {
        let text = format!("X{}", "a".repeat(length));
        let name = FieldName::try_from_bytes(text.as_bytes()).unwrap();
        assert_eq!(name.as_str(), text.to_ascii_lowercase());
        assert_ne!(name, "unrelated");
        assert!(!<FieldName as PartialEq<str>>::eq(&name, "unrelated"));
        assert!(!<str as PartialEq<FieldName>>::eq("unrelated", &name));
        let mut expected = HashWrites::default();
        text.len().hash(&mut expected);
        for byte in text.bytes() {
            byte.to_ascii_lowercase().hash(&mut expected);
        }
        assert_eq!(hash_writes(&name), expected);
    }
    for length in [0, 1, 63, 64, 65, 128] {
        let bytes = vec![b'a'; length];
        let owned = FieldValue::try_from(bytes.clone()).unwrap();
        let view = FieldValueRef::new(&bytes);
        let other = FieldValue::from_static("z");
        assert_eq!(owned.len(), length);
        assert_eq!(owned.as_bytes(), bytes);
        assert_eq!(owned, view);
        assert_ne!(owned, other);
        assert_ne!(view, other.as_field_value_ref());
        assert_ne!(owned, other.as_field_value_ref());
        assert_ne!(view, other);
        assert_ne!(other.as_field_value_ref(), owned);
        assert_ne!(other, view);
        assert_eq!(owned.cmp(&owned), Ordering::Equal);
        assert_eq!(view.cmp(&view), Ordering::Equal);
        assert_eq!(owned.cmp(&other), Ordering::Less);
        assert_eq!(other.cmp(&owned), Ordering::Greater);
        assert_eq!(view.cmp(&other.as_field_value_ref()), Ordering::Less);
        assert_eq!(other.as_field_value_ref().cmp(&view), Ordering::Greater);
        assert_eq!(hash_writes(&owned), hash_writes(bytes.as_slice()));
        assert_eq!(hash_writes(view), hash_writes(bytes.as_slice()));
        assert!(!owned.is_sensitive());
    }
}

#[test]
fn gamma_survivor_encoded_values_extend_empty_and_nonempty_and_hash_contents() {
    for count in 0..=5 {
        let expected: Vec<_> = (0..count).map(|index| FieldValue::from_str(index.to_string()).unwrap()).collect();
        let mut values = EncodedValues::new();
        values.extend(expected.clone());
        assert_eq!(values.len(), count);
        assert_eq!(values.iter().cloned().collect::<Vec<_>>(), expected);
        let mut manual = HashWrites::default();
        count.hash(&mut manual);
        for value in &expected {
            value.hash(&mut manual);
        }
        assert_eq!(hash_writes(&values), manual);
        let mut extended = values.clone();
        extended.extend([FieldValue::from_static("z")]);
        assert_ne!(extended, values);
        assert_eq!(extended.cmp(&extended), Ordering::Equal);
        assert_eq!(extended.cmp(&values), Ordering::Greater);
        assert_eq!(extended.into_iter().count(), count + 1);
    }
    let value = FieldValue::from_static("retained");
    assert_eq!(EncodedValues::from(value.clone()).iter().next(), Some(&value));
    let mut pushed = EncodedValues::new();
    pushed.push(FieldValue::from_static("first"));
    pushed.extend([FieldValue::from_static("second")]);
    assert_eq!(
        pushed.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(),
        [b"first".as_slice(), b"second"]
    );
}

#[test]
fn gamma_survivor_authorization_validates_every_token_block_and_base64_padding() {
    for length in 1..=96 {
        let token = "a".repeat(length);
        let wire = format!("Bearer  {token}");
        let source = source::<Authorization<Bearer>>(&[&wire]);
        assert_eq!(Authorization::<Bearer>::view(&source).unwrap().unwrap().token(), token.as_bytes());
        assert_eq!(
            Authorization::<Bearer>::owned(&source)
                .unwrap()
                .unwrap()
                .as_field_value()
                .as_bytes(),
            wire.as_bytes()
        );
        for index in 0..length {
            let mut invalid = token.as_bytes().to_vec();
            invalid[index] = b'@';
            let invalid = String::from_utf8(invalid).unwrap();
            rejects::<Authorization<Bearer>>(&[&format!("Bearer {invalid}")]);
        }
        accepts::<Authorization<Bearer>>(&[&format!("Bearer {token}==")]);
    }
    for invalid in [
        "Basic",
        "Basic ",
        "Other YTo=",
        "Basic Y",
        "Basic YTo?",
        "Basic ?To=",
        "Basic Y?o=",
        "Basic YQ==",
        "Basic Oh==",
        "Basic Ojp=",
    ] {
        rejects::<Authorization<Basic>>(&[invalid]);
    }
    for length in 0..=24 {
        let username = vec![b'u'; length];
        let password = vec![b'p'; length];
        let mut payload = username.clone();
        payload.push(b':');
        payload.extend_from_slice(&password);
        let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(payload));
        let owned = AuthorizationOwned::<Basic>::basic(&username, &password).unwrap();
        assert_eq!(owned.as_field_value().as_bytes(), expected.as_bytes());
        assert!(owned.as_field_value().is_sensitive());
        let source = source::<Authorization<Basic>>(&[&expected]);
        let view = Authorization::<Basic>::view(&source).unwrap().unwrap();
        let mut scratch = BasicCredentials::new();
        let credentials = view.extract(&mut scratch).unwrap();
        assert_eq!(credentials.username(), username);
        assert_eq!(credentials.password(), password);
        assert!(format!("{view:?}").starts_with("AuthorizationView {"));
        assert!(format!("{owned:?}").starts_with("AuthorizationOwned {"));
        assert!(format!("{scratch:?}").starts_with("BasicCredentials {"));
    }
}

#[test]
fn gamma_survivor_cache_directives_project_quotes_names_and_seconds() {
    for (wire, expected_name, expected_value, seconds, no_cache) in [
        ("max-age=123", "max-age", Some(b"123".as_slice()), Some(123), false),
        ("MAX-AGE=\"901\"", "MAX-AGE", Some(b"\"901\"".as_slice()), Some(901), false),
        ("max-age=\"9\\0\"", "max-age", Some(b"\"9\\0\"".as_slice()), None, false),
        ("max-age=\"\"", "max-age", Some(b"\"\"".as_slice()), None, false),
        (
            "max-age=\"18446744073709551616\"",
            "max-age",
            Some(b"\"18446744073709551616\"".as_slice()),
            None,
            false,
        ),
        ("x-age=123", "x-age", Some(b"123".as_slice()), None, false),
        ("x=\"a\\\",b\"", "x", Some(b"\"a\\\",b\"".as_slice()), None, false),
        ("no-cache=\"x\"", "no-cache", Some(b"\"x\"".as_slice()), None, true),
        ("no-cache", "no-cache", None, None, true),
        ("private", "private", None, None, false),
    ] {
        let source = source::<CacheControl>(&[wire]);
        let view = CacheControl::view(&source).unwrap().unwrap();
        let owned = CacheControl::owned(&source).unwrap().unwrap();
        assert_eq!(view.max_age(), seconds.map(Duration::from_secs), "{wire}");
        assert_eq!(owned.max_age(), seconds.map(Duration::from_secs), "{wire}");
        assert_eq!(view.no_cache(), no_cache);
        assert_eq!(owned.no_cache(), no_cache);
        for directive in [view.directives().next().unwrap(), owned.directives().next().unwrap()] {
            assert_eq!(directive.name(), expected_name);
            assert_eq!(directive.value(), expected_value);
        }
        assert_eq!(view.directives().take(3).count(), 1);
        assert_eq!(owned.directives().take(3).count(), 1);
        assert!(format!("{view:?}").starts_with("CacheControlView {"));
        assert!(format!("{owned:?}").starts_with("CacheControlOwned { value_count:"));
    }
    for wire in [
        "public",
        "private",
        "no-store",
        "immutable",
        "no-transform",
        "only-if-cached",
        "must-revalidate",
        "proxy-revalidate",
    ] {
        accepts::<CacheControl>(&[wire]);
        accepts::<CacheControl>(&[&format!("{wire}x")]);
    }
    for invalid in ["max-age=", "max-age=\"", "x=\"a\"b", "x=a b", "=x", "x=\"a\\\"", "x=\"a\"\""] {
        rejects::<CacheControl>(&[invalid]);
    }
    let source = source::<CacheControl>(&["public", "x=\"unfinished"]);
    assert_eq!(CacheControl::view(&source).unwrap_err().value_index(), Some(1));
    assert_eq!(CacheControl::owned(&source).unwrap_err().value_index(), Some(1));
}

#[test]
fn gamma_survivor_cache_builders_preserve_all_directives_and_reject_invalid_extensions() {
    let built = CacheControlOwned::builder()
        .no_store()
        .must_revalidate()
        .immutable()
        .max_age(Duration::from_secs(123_456_789))
        .extension_value("x", "\"a,b\"")
        .build()
        .unwrap();
    let mut sink = TestMap::default();
    CacheControl::insert(&mut sink, built).unwrap();
    assert_eq!(
        sink.get_all(&FieldName::CacheControl)[0].as_bytes(),
        b"no-store, must-revalidate, immutable, max-age=123456789, x=\"a,b\""
    );
    for value in ["", "two words", "\"unterminated"] {
        assert!(CacheControlOwned::builder().extension_value("x", value).build().err().is_some());
    }
    assert!(CacheControlOwned::builder().build().err().is_some());
}

#[test]
fn gamma_survivor_decimal_lengths_and_error_line_indices() {
    for number in [0, 9, 10, 99, 100, 123_456_789, 9_999_999_999_999_999_999, u64::MAX] {
        let wire = number.to_string();
        let source = source::<ContentLength>(&[&format!(" \t{wire}\t "), &wire, &format!("{wire}, {wire}")]);
        assert_eq!(ContentLength::view(&source).unwrap().unwrap().get(), number);
        assert_eq!(ContentLength::owned(&source).unwrap().unwrap().get(), number);
        let mut sink = TestMap::default();
        sink.set_content_length(number).unwrap();
        assert_eq!(sink.get_all(&FieldName::ContentLength)[0].as_bytes(), wire.as_bytes());
    }
    for index in 0..4 {
        let mut values = vec!["42"; 4];
        values[index] = "4:";
        let source = source::<ContentLength>(&values);
        for error in [
            ContentLength::view(&source).unwrap_err(),
            ContentLength::owned(&source).unwrap_err(),
        ] {
            assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
            assert_eq!(error.value_index(), Some(index));
        }
    }
    for invalid in ["", ":", "1:", "18446744073709551616", "1,,1", "1,2", "1;1"] {
        rejects::<ContentLength>(&[invalid]);
    }
}

#[test]
fn gamma_survivor_content_type_common_metadata_and_parameter_iteration() {
    for (wire, type_, subtype, count) in [
        ("application/json", "application", "json", 0),
        ("application/octet-stream", "application", "octet-stream", 0),
        ("application/javascript", "application", "javascript", 0),
        ("text/html", "text", "html", 0),
        ("text/plain", "text", "plain", 0),
        ("text/css", "text", "css", 0),
        ("application/json; charset=utf-8", "application", "json", 1),
        ("text/html; charset=utf-8", "text", "html", 1),
        ("text/plain;a=1;b=2;c=\"three\\\"four\"", "text", "plain", 3),
    ] {
        let source = source::<ContentType>(&[wire]);
        let view = ContentType::view(&source).unwrap().unwrap();
        let owned = ContentType::owned(&source).unwrap().unwrap();
        assert_eq!(view.type_().unwrap(), type_);
        assert_eq!(view.subtype().unwrap(), subtype);
        assert_eq!(owned.type_().unwrap(), type_);
        assert_eq!(owned.subtype().unwrap(), subtype);
        let mut parameters = view.parameters();
        assert_eq!(parameters.size_hint(), (0, Some(count)));
        for remaining in (0..count).rev() {
            assert!(parameters.next().is_some());
            assert_eq!(parameters.size_hint(), (0, Some(remaining)));
        }
        assert!(parameters.next().is_none());
        assert!(parameters.next().is_none());
        assert_eq!(owned.parameters().take(6).count(), count);
        for (expected, parameter) in wire.split(';').skip(1).zip(owned.parameters()) {
            let (name, value) = expected.trim().split_once('=').unwrap();
            let parameter = parameter.unwrap();
            assert_eq!(parameter.name(), name);
            assert_eq!(parameter.value(), value.as_bytes());
            assert_eq!(view.parameter(name).unwrap(), Some(value.as_bytes()));
            assert_eq!(owned.parameter(name).unwrap(), Some(value.as_bytes()));
        }
        assert_eq!(owned.parameter("absent").unwrap(), None);
        assert_eq!(view.parameter("absent").unwrap(), None);
    }
    for invalid in [
        "text",
        "text plain",
        "text/",
        "/plain",
        "text/plain a=1",
        "text/plain;a",
        "text/plain;a=\"",
        "text/plain;a=\"x\\\"",
    ] {
        rejects::<ContentType>(&[invalid]);
    }
}

#[test]
fn gamma_survivor_dates_match_independent_calendar_at_month_century_and_clock_boundaries() {
    for seconds in [
        0,
        86_399,
        86_400,
        2_678_399,
        5_097_599,
        7_775_999,
        10_367_999,
        13_046_399,
        15_638_399,
        18_316_799,
        20_995_199,
        23_587_199,
        26_265_599,
        28_857_599,
        31_535_999,
        946_684_800,
        951_782_400,
        4_107_542_400,
        13_574_649_600,
        253_402_300_799,
    ] {
        let date = UNIX_EPOCH + Duration::from_secs(seconds);
        let wire = httpdate::fmt_http_date(date);
        let source = source::<LastModified>(&[&wire]);
        assert_eq!(LastModified::view(&source).unwrap().unwrap().date(), date, "{wire}");
        assert_eq!(LastModified::owned(&source).unwrap().unwrap().date(), date, "{wire}");
        accepts::<IfModifiedSince>(&[&wire]);
        accepts::<IfUnmodifiedSince>(&[&wire]);
        accepts::<IfRange>(&[&wire]);
    }
    for day in 0..7 {
        let date = UNIX_EPOCH + Duration::from_secs(day * 86400 + 12345);
        let wire = httpdate::fmt_http_date(date);
        assert_eq!(
            LastModified::owned(&source::<LastModified>(&[&wire])).unwrap().unwrap().date(),
            date
        );
    }
    let canonical = "Thu, 01 Jan 1970 00:00:00 GMT";
    for index in [4, 16, 19, 18, 21, 24] {
        let mut invalid = canonical.as_bytes().to_vec();
        invalid[index] = if matches!(index, 18 | 21 | 24) { b':' } else { b'x' };
        rejects::<LastModified>(&[&String::from_utf8(invalid).unwrap()]);
    }
    for weekday in ["Sun", "Mon", "Tue", "Wed", "Fri", "Sat"] {
        rejects::<LastModified>(&[&format!("{weekday}, 01 Jan 1970 00:00:00 GMT")]);
    }
    for invalid in [
        "Thu, 00 Jan 1970 00:00:00 GMT",
        "Thu, 32 Jan 1970 00:00:00 GMT",
        "Thu, 01 Jan 1969 00:00:00 GMT",
        "Thu, 01 Jan 1970 24:00:00 GMT",
        "Thu, 01 Jan 1970 00:60:00 GMT",
        "Thu, 01 Jan 1970 00:00:60 GMT",
        "Thu, 01 Jan 1970 0a:00:00 GMT",
        "Thu, 01 Jan 1970 :0:00:00 GMT",
        "Thu, 01 Jan 1970 00:00:00 GMT extra",
        "Wed, 29 Feb 2100 00:00:00 GMT",
        "Fri, 30 Feb 2000 00:00:00 GMT",
    ] {
        rejects::<LastModified>(&[invalid]);
    }
}

#[test]
fn gamma_survivor_etags_compare_strength_and_scan_every_chunk() {
    for length in 0..=32 {
        let tag = "a".repeat(length);
        let strong = ETagOwned::strong(&tag).unwrap();
        let weak = ETagOwned::weak(&tag).unwrap();
        let different = ETagOwned::strong("different").unwrap();
        assert_eq!(strong.opaque_tag().unwrap(), tag.as_bytes());
        assert_eq!(weak.opaque_tag().unwrap(), tag.as_bytes());
        assert_eq!(strong.strong_eq(&weak), Ok(false));
        assert_eq!(weak.strong_eq(&strong), Ok(false));
        assert_eq!(strong.strong_eq(&different), Ok(false));
        assert_eq!(strong.weak_eq(&different), Ok(false));
        let strong_wire = format!("\"{tag}\"");
        let strong_source = source::<ETag>(&[&strong_wire]);
        let strong_view = ETag::view(&strong_source).unwrap().unwrap();
        let weak_wire = format!("W/\"{tag}\"");
        let weak_source = source::<ETag>(&[&weak_wire]);
        let weak_view = ETag::view(&weak_source).unwrap().unwrap();
        let different_source = source::<ETag>(&["\"different\""]);
        let different_view = ETag::view(&different_source).unwrap().unwrap();
        assert!(!weak_view.strong_eq(strong_view));
        assert!(!strong_view.strong_eq(different_view));
        assert!(!strong_view.weak_eq(different_view));
        assert!(strong_view.weak_eq(weak_view));
        for position in 0..length {
            let mut invalid = tag.clone().into_bytes();
            invalid[position] = b'"';
            assert!(ETagOwned::strong(String::from_utf8(invalid).unwrap()).err().is_some());
        }
    }
    rejects::<IfRange>(&["W/\"weak\""]);
}

#[test]
fn gamma_survivor_quality_hashing_and_builder_lengths_cover_every_thousandth() {
    for thousandths in 0..=1000 {
        let quality = Quality::from_thousandths(thousandths).unwrap();
        let expected = if thousandths == 1000 {
            "1".to_owned()
        } else if thousandths == 0 {
            "0".to_owned()
        } else {
            format!("0.{thousandths:03}").trim_end_matches('0').to_owned()
        };
        let compact: QualityView<'_> = quality.into();
        let wire = format!("0.{thousandths:03}");
        let parsed = QualityView::parse(if thousandths == 1000 { b"1" } else { wire.as_bytes() }, DecodeMode::Strict).unwrap();
        assert_eq!(parsed.to_quality().unwrap(), quality);
        assert_eq!(compact.to_string(), expected);
        assert_eq!(parsed.to_string(), expected);
        assert_ne!(parsed, if thousandths == 0 { QualityView::ONE } else { QualityView::ZERO });
        assert_eq!(hash_writes(compact), hash_writes(parsed));
        let mut manual = HashWrites::default();
        (thousandths == 1000).hash(&mut manual);
        let fraction = expected.split_once('.').map_or("", |(_, fraction)| fraction);
        fraction.len().hash(&mut manual);
        for byte in fraction.bytes() {
            byte.hash(&mut manual);
        }
        assert_eq!(hash_writes(compact), manual);
        let encoding = AcceptEncodingEntry::new(ContentCoding::parse("gzip").unwrap(), Some(compact)).unwrap();
        let header = AcceptEncodingOwned::from_entries([encoding, encoding]).unwrap();
        assert_eq!(
            header.values().next().unwrap().as_bytes(),
            format!("gzip;q={expected}, gzip;q={expected}").as_bytes()
        );
        let language = AcceptLanguageEntry::new(LanguageRange::parse("en-US").unwrap(), Some(compact)).unwrap();
        let header = AcceptLanguageOwned::from_entries([language, language]).unwrap();
        assert_eq!(
            header.values().next().unwrap().as_bytes(),
            format!("en-US;q={expected}, en-US;q={expected}").as_bytes()
        );
    }
}

#[test]
fn gamma_survivor_token_hashes_include_length_casefolded_bytes_and_inequality() {
    for text in ["q", "Ab", "X-Token", "abcdefghijklmnopq"] {
        let token = NegotiationToken::new(text).unwrap();
        let name = FieldNameView::new(text).unwrap();
        let mut manual = HashWrites::default();
        text.len().hash(&mut manual);
        for byte in text.bytes() {
            byte.to_ascii_lowercase().hash(&mut manual);
        }
        assert_eq!(hash_writes(token), manual);
        assert_eq!(hash_writes(name), manual);
        assert_ne!(name, FieldNameView::new("unrelated").unwrap());
    }
    assert!(!LanguageRange::parse("en-US").unwrap().eq_ignore_ascii_case("de-DE"));
}

#[test]
fn gamma_survivor_negotiation_parameters_and_strict_relaxed_errors() {
    for valid in [
        "text/plain",
        "application/json;level=1;q=0.125;extension",
        "text/plain;a=\"x\\\"y\";q=1;foo=\"a,b\"",
    ] {
        accepts::<Accept>(&[valid]);
        let source = source::<Accept>(&[valid]);
        let view = Accept::view(&source).unwrap().unwrap();
        assert_eq!(view.items().take(4).count(), 1);
        assert_eq!(view.entries().take(4).count(), 1);
    }
    for invalid in [
        "text",
        "text/",
        "text/plain;q=",
        "text/plain;q=2",
        "text/plain;q=0.5;q=0.4",
        "text/plain;a",
        "text/plain;a=\"unfinished",
    ] {
        rejects::<Accept>(&[invalid]);
    }
    for invalid in [
        "gzip;q=",
        "gzip;x=1",
        "gzip;q=0.5;x=1",
        "gzip;q=\"0.5\"",
        "gzip;q=2",
        "gzip;q=0.5;q=0.5",
    ] {
        rejects::<AcceptEncoding>(&[invalid]);
        rejects::<AcceptLanguage>(&[invalid]);
    }
    for wire in ["gzip; q = .5", "gzip;q= .5000"] {
        let source = source::<AcceptEncoding>(&[wire]);
        assert!(AcceptEncoding::view_with(&source, DecodeMode::Strict).err().is_some());
        let view = AcceptEncoding::view_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
        assert_eq!(view.entries().next().unwrap().quality().to_quality().unwrap().thousandths(), 500);
    }
    let source = source::<Accept>(&["text/plain", "text/html;a=\"unfinished"]);
    assert_eq!(Accept::view(&source).unwrap_err().value_index(), None);
    assert_eq!(Accept::owned(&source).unwrap_err().value_index(), None);
}

#[test]
fn gamma_survivor_cors_empty_lists_multiple_lines_and_case_sensitive_methods() {
    let owned = AccessControlAllowMethodsOwned::from_field_values(vec![
        FieldValue::from_static("GET"),
        FieldValue::from_static(""),
        FieldValue::from_static("POST, get"),
    ])
    .unwrap();
    assert_eq!(
        owned.iter().take(8).map(MethodView::as_str).collect::<Vec<_>>(),
        ["GET", "POST", "get"]
    );
    let mut manual = HashWrites::default();
    for value in owned.field_values() {
        value.hash(&mut manual);
    }
    assert_eq!(hash_writes(&owned), manual);
    assert_eq!(AccessControlAllowMethodsOwned::from_methods([] as [&str; 0]).unwrap().len(), 0);
    assert_eq!(
        AccessControlAllowMethodsOwned::from_field_values(Vec::new()).unwrap_err().kind(),
        DecodeErrorKind::MissingValue
    );
    assert_eq!(AccessControlAllowHeadersOwned::from_header_names([] as [&str; 0]).unwrap().len(), 0);
    assert_eq!(
        AccessControlExposeHeadersOwned::from_header_names([] as [&str; 0]).unwrap().len(),
        0
    );
    assert!(AccessControlRequestHeadersOwned::from_header_names([] as [&str; 0]).err().is_some());
    for method in [
        "GET", "PUT", "HEAD", "POST", "PATCH", "TRACE", "DELETE", "CONNECT", "OPTIONS", "get", "CUSTOM",
    ] {
        let source = source::<AccessControlRequestMethod>(&[method]);
        assert_eq!(
            AccessControlRequestMethod::view(&source).unwrap().unwrap().method().as_str(),
            method
        );
        assert_eq!(
            AccessControlRequestMethod::owned(&source)
                .unwrap()
                .unwrap()
                .method()
                .unwrap()
                .as_str(),
            method
        );
    }
    accepts::<AccessControlRequestHeaders>(&["X-First", ""]);
    rejects::<AccessControlRequestHeaders>(&["", ""]);
    rejects::<AccessControlAllowMethods>(&["GET", "bad method"]);
    rejects::<AccessControlRequestMethod>(&["GET", "POST"]);
    rejects::<AccessControlAllowCredentials>(&["TRUE"]);
    accepts::<AccessControlAllowCredentials>(&[" \ttrue\t "]);
}

#[test]
fn gamma_survivor_host_and_origin_component_ports_and_ipv6_zero_runs() {
    for address in [
        Ipv6Addr::UNSPECIFIED,
        Ipv6Addr::LOCALHOST,
        Ipv6Addr::new(1, 0, 0, 2, 0, 0, 3, 4),
        Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
    ] {
        let wire = format!("[{address}]:12345");
        let host = HostOwned::try_from(wire.as_str()).unwrap();
        assert_eq!(host.port_view().unwrap().to_u16().unwrap(), 12345);
        assert_eq!(host.as_field_value().as_bytes(), wire.as_bytes());
        accepts::<AccessControlAllowOrigin>(&[&format!("https://[{address}]:12345")]);
    }
    for port in [0, 1, 80, 443, u16::MAX] {
        let host = HostOwned::from_ipv4(Ipv4Addr::new(192, 0, 2, 1), Some(port));
        assert_eq!(host.port_view().unwrap().to_u16().unwrap(), port);
        assert_eq!(host.as_field_value().as_bytes(), format!("192.0.2.1:{port}").as_bytes());
    }
    for valid in ["example.com", "a%20b", "1.2.3.4", "[vF.address]", "example.com:65535"] {
        accepts::<Host>(&[valid]);
    }
    for invalid in ["a%0z", "a%z0", "[vG.address]", "[v1G.address]", "host:1:2", "host:port"] {
        rejects::<Host>(&[invalid]);
    }
    for valid in ["*", "null", "https://example.com", "http://a-b.example:81", "https://127.0.0.1:123"] {
        accepts::<AccessControlAllowOrigin>(&[valid]);
    }
    for invalid in [
        "https://",
        "1https://host",
        "https://-host",
        "https://host-",
        "https://host:0001",
        "https://host:65536",
        "https://host:1:2",
        "https://host/path",
    ] {
        rejects::<AccessControlAllowOrigin>(&[invalid]);
    }
}

#[test]
fn gamma_survivor_location_components_hash_redaction_and_constructed_lengths() {
    for (scheme, authority, path, query, fragment) in [
        (Some("https"), Some("user@host:123"), "/a/b", Some("q=1"), Some("fragment")),
        (None, Some("host"), "/a", None, None),
        (None, None, "../a", Some(""), Some("")),
        (Some("foo"), None, "a:b", None, None),
    ] {
        let authority = authority.map(|text| {
            let (userinfo, rest) = text.split_once('@').map_or((None, text), |(user, rest)| (Some(user), rest));
            let (host, port) = rest.split_once(':').map_or((rest, None), |(host, port)| (host, Some(port)));
            UriAuthority::new(userinfo, host, port).unwrap()
        });
        let expected = format!(
            "{}{}{}{}{}",
            scheme.map_or(String::new(), |text| format!("{text}:")),
            authority.map_or(String::new(), |text| format!(
                "//{}{}{}",
                text.userinfo().map_or(String::new(), |user| format!("{user}@")),
                text.host(),
                text.port().map_or(String::new(), |port| format!(":{port}"))
            )),
            path,
            query.map_or(String::new(), |text| format!("?{text}")),
            fragment.map_or(String::new(), |text| format!("#{text}"))
        );
        let owned = LocationOwned::from_components(scheme, authority, path, query, fragment).unwrap();
        assert_eq!(owned.as_bytes(), expected.as_bytes());
        assert!(owned.clone().into_field_value().is_sensitive());
        let parsed = owned.uri_reference();
        assert_eq!(parsed.scheme(), scheme);
        assert_eq!(parsed.path(), path);
        assert_eq!(parsed.query(), query);
        assert_eq!(parsed.fragment(), fragment);
        assert_eq!(hash_writes(parsed), hash_writes(parsed.as_str()));
        assert_eq!(format!("{parsed:?}"), "UriReference { redacted: true, .. }");
        assert_eq!(format!("{owned:?}"), "LocationOwned { redacted: true, .. }");
    }
    let backslash = source::<Location>(&["/a\\b"]);
    assert!(Location::view(&backslash).err().is_some());
    assert_eq!(
        Location::view_with(&backslash, DecodeMode::Relaxed)
            .unwrap()
            .unwrap()
            .uri_reference()
            .path(),
        "/a/b"
    );
    for invalid in ["/bad%2", "/a b", "//[v1G.address]", "a:b"] {
        if invalid == "a:b" {
            assert!(LocationOwned::from_components(None, None, invalid, None, None).err().is_some());
        } else {
            rejects::<Location>(&[invalid]);
        }
    }
}

#[test]
fn gamma_survivor_range_iterator_boundaries_and_invalid_delimiters() {
    for (wire, expected) in [
        ("bytes=0-0", vec![ByteRangeSpec::FromTo { first: 0, last: 0 }]),
        (
            "bytes=0-1, 2-3",
            vec![
                ByteRangeSpec::FromTo { first: 0, last: 1 },
                ByteRangeSpec::FromTo { first: 2, last: 3 },
            ],
        ),
        (
            "bytes=9-, -1",
            vec![ByteRangeSpec::From { first: 9 }, ByteRangeSpec::Suffix { length: 1 }],
        ),
        (
            "bytes=10-11, 123-456",
            vec![
                ByteRangeSpec::FromTo { first: 10, last: 11 },
                ByteRangeSpec::FromTo { first: 123, last: 456 },
            ],
        ),
    ] {
        let source = source::<Range>(&[wire]);
        assert_eq!(
            Range::view(&source)
                .unwrap()
                .unwrap()
                .byte_ranges()
                .unwrap()
                .take(8)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            Range::owned(&source)
                .unwrap()
                .unwrap()
                .byte_ranges()
                .unwrap()
                .take(8)
                .collect::<Vec<_>>(),
            expected
        );
    }
    for invalid in ["bytes=1-0", "bytes=1-2-3", "bytes=-", "bytes=0-1x"] {
        rejects::<Range>(&[invalid]);
    }
    for valid in ["bytes=01-2", "bytes=0-01", "bytes=0-1,"] {
        accepts::<Range>(&[valid]);
    }
    for invalid in ["bytes 0-1/2/3", "bytes 0-1-2/3", "bytes */2/3", "bytes 0-1/1"] {
        rejects::<ContentRange>(&[invalid]);
    }
    let owned = AcceptRangesOwned::from_units(["bytes", "items"]).unwrap();
    assert_eq!(owned.to_string(), "bytes, items");
    assert_eq!(owned.units().take(6).collect::<Vec<_>>(), ["bytes", "items"]);
    accepts::<AcceptRanges>(&["bytes"]);
    accepts::<AcceptRanges>(&["none"]);
    rejects::<AcceptRanges>(&["none, bytes"]);
    accepts::<AcceptRanges>(&["none", ""]);
}

#[test]
fn gamma_survivor_policy_iteration_flags_quotes_and_builder_extensions() {
    for (wire, age, subdomains, preload) in [
        ("max-age=31536000; includeSubDomains", 31_536_000, true, false),
        ("max-age=63072000; includeSubDomains; preload", 63_072_000, true, true),
        ("max-age=9", 9, false, false),
        (" \tmax-age=\"123\" ; x=\"a\\\"b\" ; preload ", 123, false, true),
    ] {
        let source = source::<StrictTransportSecurity>(&[wire]);
        let view = StrictTransportSecurity::view(&source).unwrap().unwrap();
        let owned = StrictTransportSecurity::owned(&source).unwrap().unwrap();
        assert_eq!(view.max_age(), Duration::from_secs(age));
        assert_eq!(owned.max_age(), Duration::from_secs(age));
        assert_eq!(view.include_subdomains(), subdomains);
        assert_eq!(owned.include_subdomains(), subdomains);
        assert_eq!(view.preload(), preload);
        assert_eq!(owned.preload(), preload);
    }
    for invalid in [
        "max-age=",
        "max-age=\"",
        "max-age=1; x=\"",
        "max-age=1; x=\"a\\\"",
        "max-age=1; x=\"a\"b",
    ] {
        rejects::<StrictTransportSecurity>(&[invalid]);
    }
    let source = source::<ReferrerPolicy>(&["origin, unsafe-url", "strict-origin"]);
    let view = ReferrerPolicy::view(&source).unwrap().unwrap();
    assert_eq!(
        view.tokens().take(6).map(|token| token.unwrap().as_str()).collect::<Vec<_>>(),
        ["origin", "unsafe-url", "strict-origin"]
    );
    rejects::<ReferrerPolicy>(&["bad token", "origin"]);
    rejects::<ReferrerPolicy>(&["origin", "bad token"]);
}

#[test]
fn gamma_survivor_websocket_extensions_parameters_and_version_set_word_iteration() {
    for wire in [
        "permessage-deflate",
        "permessage-deflate; client_max_window_bits",
        "permessage-deflate; server_no_context_takeover; client_no_context_takeover",
        ", , custom; p=one; q=\"two\\hthree\", other; flag",
    ] {
        accepts::<SecWebSocketExtensions>(&[wire]);
        let source = source::<SecWebSocketExtensions>(&[wire]);
        let view = SecWebSocketExtensions::view(&source).unwrap().unwrap();
        let expected_count = if wire.starts_with(',') { 2 } else { 1 };
        assert_eq!(view.extensions().take(6).count(), expected_count);
        assert_eq!(
            SecWebSocketExtensions::owned(&source)
                .unwrap()
                .unwrap()
                .extensions()
                .take(6)
                .count(),
            expected_count
        );
    }
    for invalid in [
        "",
        ",,",
        "x;=a",
        "x;p=",
        "x;p=\"",
        "x;p=\"a\\\"",
        "x;p=\"a\"b",
        "x;;p=a",
        "x; bad name=value",
    ] {
        rejects::<SecWebSocketExtensions>(&[invalid]);
    }
    for wire in [
        "graphql-ws",
        "chat, superchat",
        "graphql-transport-ws",
        "graphql-transport-ws, graphql-ws",
        "a, b, c",
    ] {
        accepts::<SecWebSocketProtocol>(&[wire]);
        let source = source::<SecWebSocketProtocol>(&[wire]);
        let expected: Vec<_> = wire.split(", ").collect();
        assert_eq!(
            SecWebSocketProtocol::view(&source)
                .unwrap()
                .unwrap()
                .protocols()
                .take(8)
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            expected
        );
    }
    let wire = (0..=255).rev().map(|value| value.to_string()).collect::<Vec<_>>().join(", ");
    let source = source::<SecWebSocketVersion>(&[&wire]);
    let expected: Vec<u8> = (0..=255).collect();
    let view = SecWebSocketVersion::view(&source).unwrap().unwrap();
    assert_eq!(view.versions().take(257).collect::<Vec<_>>(), expected);
    assert_eq!(
        view.to_string(),
        (0..=255).map(|value| value.to_string()).collect::<Vec<_>>().join(", ")
    );
    for invalid in ["256", "013", "13x", "13, \"unfinished"] {
        rejects::<SecWebSocketVersion>(&[invalid]);
    }
}

struct FailBytes(usize);

impl fmt::Write for FailBytes {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 = self.0.checked_sub(s.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

fn formatting_errors(value: impl fmt::Display) {
    let length = value.to_string().len();
    for budget in 0..length {
        assert_eq!(write!(&mut FailBytes(budget), "{value}"), Err(fmt::Error));
    }
}

#[test]
fn gamma_survivor_display_propagates_failure_at_every_output_byte_budget() {
    formatting_errors(AcceptRangesOwned::from_units(["bytes", "items"]).unwrap());
    formatting_errors(AccessControlAllowMethodsOwned::from_methods(["GET", "POST"]).unwrap());
    formatting_errors(AccessControlAllowOriginOwned::try_from("https://example.com").unwrap());
    formatting_errors(AccessControlRequestMethodOwned::try_from("CUSTOM").unwrap());
    formatting_errors(QualityView::parse(b"0.125", DecodeMode::Strict).unwrap());
    formatting_errors(
        SecWebSocketVersion::owned(&source::<SecWebSocketVersion>(&["8, 13, 255"]))
            .unwrap()
            .unwrap(),
    );
    formatting_errors(
        ReferrerPolicy::owned(&source::<ReferrerPolicy>(&["origin", "unsafe-url"]))
            .unwrap()
            .unwrap(),
    );
}

struct WrongLength {
    announced: usize,
    bytes: Vec<u8>,
}

impl FieldEncoder for WrongLength {
    fn encode<O: FieldEncodeOutput>(self, output: &mut O) -> Result<(), InsertError> {
        let mut writer = output.begin_value(self.announced, FieldSensitivity::Sensitive)?;
        writer.write_bytes(&self.bytes)?;
        writer.finish()
    }
}

#[test]
fn gamma_survivor_encoder_enforces_announced_length_and_preserves_sink_on_errors() {
    for length in [0, 1, 63, 64, 65, 128] {
        let mut sink = TestMap::default();
        let value = vec![b'a'; length];
        sink.set_encoded(
            &FieldName::UserAgent,
            WrongLength {
                announced: length,
                bytes: value.clone(),
            },
        )
        .unwrap();
        assert_eq!(sink.get_all(&FieldName::UserAgent)[0].as_bytes(), value);
        assert!(sink.get_all(&FieldName::UserAgent)[0].is_sensitive());
        for announced in [length.saturating_sub(1), length + 1] {
            if announced == length {
                continue;
            }
            let error = sink
                .set_encoded(
                    &FieldName::UserAgent,
                    WrongLength {
                        announced,
                        bytes: value.clone(),
                    },
                )
                .unwrap_err();
            assert_eq!(error.kind(), InsertErrorKind::InvalidEncoding);
            assert_eq!(sink.get_all(&FieldName::UserAgent)[0].as_bytes(), value);
        }
    }
    let mut sink = TestMap::default();
    assert_eq!(
        sink.set_encoded(
            &FieldName::UserAgent,
            WrongLength {
                announced: usize::MAX,
                bytes: Vec::new()
            }
        )
        .unwrap_err()
        .kind(),
        InsertErrorKind::CapacityExceeded
    );
}

#[test]
fn gamma_survivor_sensitive_deferred_location_and_cookie_writes_keep_values() {
    let mut sink = TestMap::default();
    LocationOwned::try_from("/private?token=abc")
        .unwrap()
        .insert_into(&mut sink)
        .unwrap();
    assert_eq!(sink.get_all(&FieldName::Location)[0].as_bytes(), b"/private?token=abc");
    assert!(sink.get_all(&FieldName::Location)[0].is_sensitive());
    let mut cookies = SetCookieOwned::new();
    cookies.push_str("a=1").unwrap();
    cookies.push_str("b=2").unwrap();
    cookies.insert_into(&mut sink).unwrap();
    assert_eq!(
        sink.get_all(&FieldName::SetCookie)
            .iter()
            .map(FieldValue::as_bytes)
            .collect::<Vec<_>>(),
        [b"a=1", b"b=2"]
    );
    assert!(sink.get_all(&FieldName::SetCookie).iter().all(FieldValue::is_sensitive));
    assert!(sink.contains(&FieldName::Location));
    assert!(!sink.contains(&FieldName::Host));
}

struct RawSource<'a> {
    name: &'static FieldName,
    values: Vec<FieldValueRef<'a>>,
}

impl FieldSource for RawSource<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_borrowed(name, &self.values)).flatten()
    }
}

fn limit_kind<H: Field>(wire: &str) -> (Option<DecodeErrorKind>, Option<DecodeErrorKind>) {
    let source = source::<H>(&[wire]);
    (
        H::view(&source).err().map(|error| error.kind()),
        H::owned(&source).err().map(|error| error.kind()),
    )
}

#[test]
fn gamma_survivor_source_list_budgets_delimiters_empty_members_and_quotes() {
    macro_rules! check {
        ($header:ty, $item:literal, $delimiter:literal) => {
            let boundary = std::iter::repeat_n($item, 1024).collect::<Vec<_>>().join($delimiter);
            assert_ne!(limit_kind::<$header>(&boundary).0, Some(DecodeErrorKind::SourceLimitExceeded));
            assert_ne!(limit_kind::<$header>(&boundary).1, Some(DecodeErrorKind::SourceLimitExceeded));
            let overflow = format!("{boundary}{}{}", $delimiter, $item);
            assert_eq!(
                limit_kind::<$header>(&overflow),
                (
                    Some(DecodeErrorKind::SourceLimitExceeded),
                    Some(DecodeErrorKind::SourceLimitExceeded)
                )
            );
        };
    }
    check!(CacheControl, "public", ",");
    check!(ContentLength, "1", ",");
    check!(AcceptRanges, "bytes", ",");
    check!(AccessControlAllowMethods, "GET", ",");
    check!(Accept, "text/plain", ",");
    check!(AcceptEncoding, "gzip", ",");
    check!(IfMatch, "\"x\"", ",");
    check!(SecWebSocketProtocol, "x", ",");
    check!(SecWebSocketVersion, "13", ",");
    check!(SecWebSocketExtensions, "x", ",");
    check!(ContentType, "x=a", ";");
    check!(StrictTransportSecurity, "x", ";");
    for wire in [
        format!("{}public{}", ",".repeat(1100), ",".repeat(1100)),
        std::iter::repeat_n("x=\"a\\\",b\"", 1024).collect::<Vec<_>>().join(","),
    ] {
        accepts::<CacheControl>(&[&wire]);
    }
    accepts::<IfMatch>(&[&format!("{}\"tag\"{}", ",".repeat(1100), ",".repeat(1100))]);
    let empty_semicolons = format!("text/plain{}", ";".repeat(1024));
    assert_eq!(
        limit_kind::<ContentType>(&empty_semicolons),
        (
            Some(DecodeErrorKind::SourceLimitExceeded),
            Some(DecodeErrorKind::SourceLimitExceeded)
        )
    );
    let quoted_tag = std::iter::repeat_n("\"a\\\"", 1024).collect::<Vec<_>>().join(",");
    accepts::<IfMatch>(&[&quoted_tag]);
    for item in ["x=\"a\\\",b\"", "x\\"] {
        let overflow = std::iter::repeat_n(item, 1025).collect::<Vec<_>>().join(",");
        assert_eq!(
            limit_kind::<CacheControl>(&overflow),
            (
                Some(DecodeErrorKind::SourceLimitExceeded),
                Some(DecodeErrorKind::SourceLimitExceeded)
            )
        );
    }
}

#[test]
fn gamma_survivor_field_lines_delimited_items_start_offsets_and_fused_errors() {
    let values = [FieldValue::from_static(" a, \"b\\\",c\",, d"), FieldValue::from_static("e, f")];
    let lines = FieldLines::from_slice(&FieldName::Vary, &values).unwrap();
    assert_eq!(lines.len(), 2);
    let mut items = lines.comma_items();
    for expected in [b"a".as_slice(), b"\"b\\\",c\"", b"d", b"e", b"f"] {
        assert_eq!(items.next().unwrap().unwrap(), expected);
    }
    assert!(items.next().is_none());
    assert!(items.next().is_none());
    let line = FieldLines::single(&FieldName::Vary, b"first; \"second;part\";third");
    assert_eq!(
        line.semicolon_items().map(Result::unwrap).collect::<Vec<_>>(),
        [b"first".as_slice(), b"\"second;part\"", b"third"]
    );
    let line = FieldLines::single(&FieldName::Vary, b"a,\"unfinished");
    let mut items = line.comma_items();
    assert_eq!(items.next().unwrap().unwrap(), b"a");
    assert_eq!(items.next().unwrap().unwrap_err().kind(), DecodeErrorKind::UnterminatedQuote);
    assert!(items.next().is_none());
    assert!(items.next().is_none());
}

fn raw_rejected<H: Field>(bytes: &[u8]) {
    let source = RawSource {
        name: H::name(),
        values: vec![FieldValueRef::new(bytes)],
    };
    assert_eq!(
        H::view(&source).err().expect("forbidden raw field bytes must reject a view").kind(),
        DecodeErrorKind::InvalidSyntax
    );
    assert_eq!(
        H::owned(&source)
            .err()
            .expect("forbidden raw field bytes must reject an owned header")
            .kind(),
        DecodeErrorKind::InvalidSyntax
    );
}

#[test]
fn gamma_survivor_raw_sources_reject_forbidden_field_bytes_before_parsing() {
    for byte in [0, b'\r', b'\n', 0x1f, 0x7f] {
        let wire = [b'a', byte, b'b'];
        raw_rejected::<ContentType>(&wire);
        raw_rejected::<CacheControl>(&wire);
        raw_rejected::<ContentSecurityPolicy>(&wire);
        raw_rejected::<SetCookie>(&wire);
        raw_rejected::<AccessControlAllowOrigin>(&wire);
        raw_rejected::<SecWebSocketProtocol>(&wire);
        raw_rejected::<SecWebSocketExtensions>(&wire);
    }
}

#[test]
fn gamma_survivor_accept_component_builders_check_lengths_and_parameter_roles() {
    let range = MediaRange::parse("text/plain").unwrap();
    let name = NegotiationToken::new("level").unwrap();
    let parameter = NegotiationParameter::new(name, Some(NegotiationParameterValue::parse(b"123").unwrap()));
    let flag = NegotiationParameter::new(NegotiationToken::new("flag").unwrap(), None);
    let q = NegotiationParameter::new(NegotiationToken::new("q").unwrap(), None);
    let parameters = [parameter];
    let extensions = [flag];
    let entry = AcceptEntry::new(range, &parameters, Some(QualityView::ONE), &extensions).unwrap();
    let built = AcceptOwned::from_entries([entry, entry]).unwrap();
    assert_eq!(
        built.values().next().unwrap().as_bytes(),
        b"text/plain;level=123;q=1;flag, text/plain;level=123;q=1;flag"
    );
    assert_eq!(
        built.entries().next().unwrap().parameters().next().unwrap().name().as_str(),
        "level"
    );
    assert_eq!(built.entries().next().unwrap().extensions().next().unwrap().name().as_str(), "flag");
    assert!(AcceptEntry::new(range, &[flag], None, &[]).err().is_some());
    assert!(AcceptEntry::new(range, &[q], Some(QualityView::ONE), &[]).err().is_some());
    assert!(AcceptEntry::new(range, &[], Some(QualityView::ONE), &[q]).err().is_some());
    assert!(AcceptEntry::new(range, &[], None, &[flag]).err().is_some());
}

#[cfg(feature = "http")]
#[test]
fn gamma_survivor_http_adapter_sensitive_storage_and_announced_lengths() {
    for length in [1, 63, 64, 65, 128] {
        let mut value = http::HeaderValue::from_bytes(&vec![b'a'; length]).unwrap();
        value.set_sensitive(true);
        let owned = FieldValue::from(value.clone());
        assert!(owned.is_sensitive());
        assert_eq!(owned.as_bytes(), value.as_bytes());
        let restored = http::HeaderValue::try_from(owned).unwrap();
        assert_eq!(restored.as_bytes(), value.as_bytes());
        assert!(restored.is_sensitive());
        let mut map = http::HeaderMap::new();
        map.set_encoded(
            &FieldName::UserAgent,
            WrongLength {
                announced: length,
                bytes: vec![b'a'; length],
            },
        )
        .unwrap();
        assert_eq!(map.get("user-agent").unwrap().as_bytes(), vec![b'a'; length]);
        assert!(map.get("user-agent").unwrap().is_sensitive());
    }
    let mut map = http::HeaderMap::new();
    map.set_values(
        &FieldName::Vary,
        ["Origin", "Accept"].into_iter().map(FieldValue::from_static).collect(),
    )
    .unwrap();
    assert_eq!(
        map.get_all("vary").iter().map(http::HeaderValue::as_bytes).collect::<Vec<_>>(),
        [b"Origin".as_slice(), b"Accept"]
    );
    assert_eq!(
        map.set_encoded(
            &FieldName::Vary,
            WrongLength {
                announced: 1,
                bytes: b"too long".to_vec()
            }
        )
        .unwrap_err()
        .kind(),
        InsertErrorKind::InvalidEncoding
    );
    assert_eq!(map.get_all("vary").iter().count(), 2);
}

#[cfg(feature = "serde")]
#[test]
fn gamma_survivor_serde_error_messages_duplicates_missing_fields_and_payloads() {
    for (json, message) in [
        ("{}", "missing field `bytes`"),
        (r#"{"bytes":[]}"#, "missing field `sensitivity`"),
        (r#"{"bytes":[],"bytes":[],"sensitivity":"NonSensitive"}"#, "duplicate field `bytes`"),
        (
            r#"{"bytes":[],"sensitivity":"Sensitive","sensitivity":"NonSensitive"}"#,
            "duplicate field `sensitivity`",
        ),
        (r#"{"bytes":"x","sensitivity":"NonSensitive"}"#, "field-value bytes"),
        ("[]", "invalid length 0"),
        ("[[]]", "invalid length 1"),
        ("true", "a field value with bytes and sensitivity"),
    ] {
        assert!(
            serde_json::from_str::<FieldValue>(json).unwrap_err().to_string().contains(message),
            "{json}"
        );
    }
    for json in [
        r#"{"bytes":[1,],"sensitivity":"NonSensitive"}"#,
        r#"{"bytes":[],"sensitivity":5}"#,
        r#"{"unknown":}"#,
    ] {
        assert!(serde_json::from_str::<FieldValue>(json).err().is_some());
    }
    let value: FieldValue = serde_json::from_str(r#"{"unknown":0,"bytes":[97,98],"sensitivity":"Sensitive"}"#).unwrap();
    assert_eq!(value.as_bytes(), b"ab");
    assert!(value.is_sensitive());
    assert!(
        serde_json::from_str::<CacheControlOwned>("[]")
            .unwrap_err()
            .to_string()
            .contains("an owned header must contain at least one field value")
    );
    assert!(
        serde_json::from_str::<EncodedValues>("true")
            .unwrap_err()
            .to_string()
            .contains("a sequence of field values")
    );
    let wire = std::iter::repeat_n("public", 1025).collect::<Vec<_>>().join(",");
    let json = serde_json::to_string(&[FieldValue::from_str(&wire).unwrap()]).unwrap();
    assert!(
        serde_json::from_str::<CacheControlOwned>(&json)
            .unwrap_err()
            .to_string()
            .contains("source limit exceeded")
    );
}

#[test]
fn gamma_survivor_conditional_tag_lists_keep_order_and_reject_partial_prefixes() {
    for count in 1..=5 {
        let tags = (0..count).map(|index| ETagOwned::strong(index.to_string()).unwrap());
        let owned = IfMatchOwned::from_tags(tags).unwrap();
        assert_eq!(
            owned.tags().take(8).map(|tag| tag.opaque_tag().to_vec()).collect::<Vec<_>>(),
            (0..count).map(|index| index.to_string().into_bytes()).collect::<Vec<_>>()
        );
        assert_eq!(
            format!("{owned:?}"),
            format!("IfMatchOwned {{ value_count: {count}, wildcard: false }}")
        );
        let mut sink = TestMap::default();
        IfMatch::insert(&mut sink, owned).unwrap();
        assert_eq!(sink.get_all(&FieldName::IfMatch).len(), count);
    }
    for wire in [" \t\"a\", W/\"b\", \"c\" ", "\"a\",, \"b\"", "\"a\\\", \"b\""] {
        accepts::<IfMatch>(&[wire]);
        let source = source::<IfMatch>(&[wire]);
        assert_eq!(
            IfMatch::view(&source).unwrap().unwrap().tags().take(8).count(),
            if wire.contains('c') { 3 } else { 2 }
        );
    }
    for invalid in ["W\"a\"", "W/", "Wa", "\"a\" x", "*,\"a\"", "x", "\"unterminated"] {
        rejects::<IfMatch>(&[invalid]);
        rejects::<IfNoneMatch>(&[invalid]);
    }
}

#[test]
fn gamma_survivor_websocket_builder_errors_and_quoted_parameter_lengths() {
    let built = SecWebSocketExtensionsOwned::builder()
        .extension("custom")
        .quoted_parameter("mode", "fast")
        .extension("other")
        .parameter_flag("flag")
        .build()
        .unwrap();
    assert_eq!(built.to_string(), "custom; mode=\"fast\", other; flag");
    let first = built.extensions().next().unwrap().unwrap();
    assert_eq!(first.name(), "custom");
    let parameter = first.parameters().next().unwrap().unwrap();
    assert_eq!(parameter.name(), "mode");
    assert_eq!(parameter.value(), Some(b"\"fast\"".as_slice()));
    for builder in [
        SecWebSocketExtensionsOwned::builder(),
        SecWebSocketExtensionsOwned::builder().quoted_parameter("mode", "fast"),
        SecWebSocketExtensionsOwned::builder()
            .extension("x")
            .quoted_parameter("bad name", "fast"),
        SecWebSocketExtensionsOwned::builder()
            .extension("x")
            .quoted_parameter("mode", "bad value"),
    ] {
        assert!(builder.build().err().is_some());
    }
    let wire = "custom; p=\"ab\\c\"; q=token,other";
    let source = source::<SecWebSocketExtensions>(&[wire]);
    let view = SecWebSocketExtensions::view(&source).unwrap().unwrap();
    let extension = view.extensions().next().unwrap().unwrap();
    assert_eq!(extension.name(), "custom");
    let parameters = extension.parameters().map(Result::unwrap).collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0].value(), Some(b"\"ab\\c\"".as_slice()));
    assert_eq!(parameters[1].value(), Some(b"token".as_slice()));
}

#[test]
fn gamma_survivor_hsts_builders_and_preserved_nosniff_wire() {
    let built = StrictTransportSecurityOwned::builder(Duration::from_secs(12345))
        .include_subdomains()
        .preload()
        .extension_flag("flag")
        .extension_value("custom", "\"ab\\c\"")
        .build()
        .unwrap();
    assert_eq!(
        built.as_field_value().as_bytes(),
        b"max-age=12345; includeSubDomains; preload; flag; custom=\"ab\\c\""
    );
    for name in ["max-age", "includeSubDomains", "preload", "bad name"] {
        assert!(
            StrictTransportSecurityOwned::builder(Duration::from_secs(1))
                .extension_flag(name)
                .build()
                .err()
                .is_some()
        );
    }
    let mut value = FieldValue::from_static("nosniff");
    value.set_sensitivity(FieldSensitivity::Sensitive);
    let value = XContentTypeOptionsOwned::try_from(value).unwrap().into_field_value();
    assert_eq!(value.as_bytes(), b"nosniff");
    assert!(value.is_sensitive());
}

#[test]
fn gamma_survivor_host_constructors_retain_numeric_ports_and_future_version() {
    for port in [0, 1, 443, 12345, 65535] {
        let host = HostOwned::with_port("example.com", port).unwrap();
        assert_eq!(host.network_port().unwrap(), Some(port));
        assert_eq!(host.port().unwrap().unwrap(), port.to_string());
        assert_eq!(host.host().unwrap(), "example.com");
    }
    let future = HostOwned::try_from("[vFA.address:part]").unwrap();
    let host = HostOwned::from_parts(future.kind(), Some(HostPortView::new("123").unwrap())).unwrap();
    assert_eq!(host.host().unwrap(), "[vFA.address:part]");
    assert_eq!(host.network_port().unwrap(), Some(123));
    for invalid in ["G1", "1G"] {
        assert!(HostOwned::try_from(format!("[v{invalid}.address]")).err().is_some());
    }
    assert!(HostOwned::new("host:123").err().is_some());
    HostOwned::new("bad host").unwrap_err();
    HostPortView::new("bad").unwrap_err();
    UriAuthority::new(None, "[vFAaddress]", None).unwrap_err();
}

#[test]
fn gamma_survivor_bounded_negotiation_builder_lengths_at_the_byte_limit() {
    for (length, accepted) in [(65531, true), (65532, true), (65533, false)] {
        let coding = "a".repeat(length);
        let result = AcceptEncodingEntry::new(ContentCoding::parse(&coding).unwrap(), Some(QualityView::ONE))
            .and_then(|entry| AcceptEncodingOwned::from_entries([entry]));
        assert_eq!(
            result.map(|header| header.values().next().unwrap().as_bytes().len()).ok(),
            accepted.then_some(length + 4)
        );
    }
    let type_ = "a".repeat(65534);
    let range = MediaRange::new(NegotiationToken::new(&type_).unwrap(), NegotiationToken::new("b").unwrap()).unwrap();
    let entry = AcceptEntry::new(range, &[], None, &[]).unwrap();
    let built = AcceptOwned::from_entries([entry]).unwrap();
    assert_eq!(built.values().next().unwrap().as_bytes().len(), 65536);
}

#[test]
fn gamma_survivor_debug_type_names_and_field_labels_are_stable_and_redacted() {
    macro_rules! debug {
        ($header:ty, $wire:literal, $owned_prefix:literal, $view_prefix:literal, $field:literal) => {
            let source = source::<$header>(&[$wire]);
            let owned = format!("{:?}", <$header>::owned(&source).unwrap().unwrap());
            let view = format!("{:?}", <$header>::view(&source).unwrap().unwrap());
            assert!(owned.starts_with($owned_prefix), "{owned}");
            assert!(view.starts_with($view_prefix), "{view}");
            assert!(view.contains($field), "{view}");
        };
    }
    debug!(
        CacheControl,
        "public", "CacheControlOwned { value_count:", "CacheControlView {", "summary:"
    );
    debug!(IfRange, "\"a\"", "IfRangeOwned { value:", "IfRangeView {", "value:");
    debug!(
        AccessControlAllowOrigin,
        "null", "AccessControlAllowOriginOwned { kind:", "AccessControlAllowOriginView { kind:", "kind:"
    );
    debug!(
        AcceptRanges,
        "bytes", "AcceptRangesOwned { value_count:", "AcceptRangesView {", "none:"
    );
    debug!(
        ContentRange,
        "bytes 0-1/2", "ContentRangeOwned { unit:", "ContentRangeView { unit:", "byte_range:"
    );
    debug!(Range, "bytes=0-1", "RangeOwned { unit:", "RangeView { unit:", "unit:");
    debug!(
        SetCookie,
        "a=1", "SetCookieOwned { value_count: 1", "SetCookieView { value_count: 1", "value_count:"
    );
    debug!(
        Location,
        "/private?secret=1", "LocationOwned { redacted: true", "LocationView { redacted: true", "redacted:"
    );
    let authority = UriAuthority::new(Some("secret"), "host", Some("1")).unwrap();
    assert_eq!(format!("{authority:?}"), "UriAuthority { redacted: true, .. }");
}

struct FailOutput(usize);

struct FailValueWriter<'a>(&'a mut FailOutput);

impl FailOutput {
    fn step(&mut self) -> Result<(), InsertError> {
        self.0 = self
            .0
            .checked_sub(1)
            .ok_or_else(|| InsertError::new(InsertErrorKind::CapacityExceeded))?;
        Ok(())
    }
}

impl FieldEncodeOutput for FailOutput {
    type Writer<'a> = FailValueWriter<'a>;

    fn begin_value(&mut self, _length: usize, _sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError> {
        self.step()?;
        Ok(FailValueWriter(self))
    }

    fn push_value(&mut self, _value: FieldValue) -> Result<(), InsertError> {
        self.step()
    }
}

impl FieldValueWriter for FailValueWriter<'_> {
    fn write_bytes(&mut self, _bytes: &[u8]) -> Result<(), InsertError> {
        self.0.step()
    }

    fn finish(self) -> Result<(), InsertError> {
        self.0.step()
    }
}

fn outgoing_cache() -> CacheControlBuilder {
    CacheControlOwned::builder()
        .no_store()
        .max_age(Duration::from_secs(123_456_789))
        .extension_value("custom", "\"a,b\"")
}

#[test]
fn gamma_survivor_cache_encoding_propagates_each_output_stage_failure() {
    assert_eq!(
        http_headers::sink::ValueRefsEncoder::new([FieldValueRef::new(b"a")])
            .with_sensitivity(FieldSensitivity::Sensitive)
            .encode(&mut FailOutput(0)),
        Err(InsertError::new(InsertErrorKind::CapacityExceeded))
    );
    let mut output = FailOutput(100);
    outgoing_cache().encode(&mut output).unwrap();
    let steps = 100 - output.0;
    assert!(steps > 8);
    for allowance in 0..steps {
        assert_eq!(
            outgoing_cache().encode(&mut FailOutput(allowance)),
            Err(InsertError::new(InsertErrorKind::CapacityExceeded))
        );
    }
    for invalid in ["", "public,,private", "x=\"unterminated", "bad name=value"] {
        assert!(CacheControlOwned::try_from(invalid).err().is_some());
    }
    for length in [7, 8, 12, 14, 16] {
        let value = "a".repeat(length - 2);
        let wire = format!("x={value}");
        let cache = CacheControlOwned::try_from(wire.as_str()).unwrap();
        let item = cache.directives().next().unwrap();
        assert_eq!(item.name(), "x");
        assert_eq!(item.value(), Some(value.as_bytes()));
        rejects::<CacheControl>(&[&format!("{}@", "a".repeat(length - 1))]);
    }
}

#[cfg(feature = "http")]
#[test]
fn gamma_survivor_trusted_http_lists_report_error_indices_and_hash_each_line() {
    let mut malformed_weight = http::HeaderMap::new();
    malformed_weight.insert("accept-encoding", http::HeaderValue::from_static("gzip,identity;q="));
    assert_eq!(
        AcceptEncoding::view(&malformed_weight).unwrap_err().kind(),
        DecodeErrorKind::InvalidSyntax
    );
    let mut map = http::HeaderMap::new();
    map.append("allow", http::HeaderValue::from_static("GET"));
    map.append("allow", http::HeaderValue::from_static("POST, PUT"));
    let owned = Allow::owned(&map).unwrap().unwrap();
    assert_eq!(owned.values().len(), 2);
    assert_eq!(owned.methods().map(MethodView::as_str).collect::<Vec<_>>(), ["GET", "POST", "PUT"]);
    let mut manual = HashWrites::default();
    let stored: Vec<_> = owned.values().map(|value| value.try_to_field_value().unwrap()).collect();
    stored.as_slice().hash(&mut manual);
    assert_eq!(hash_writes(&owned), manual);
    for index in 0..3 {
        let mut map = http::HeaderMap::new();
        for line in 0..3 {
            map.append(
                "allow",
                http::HeaderValue::from_static(if line == index { "\"unfinished" } else { "GET" }),
            );
        }
        assert_eq!(Allow::view(&map).unwrap_err().value_index(), Some(index));
        assert_eq!(Allow::owned(&map).unwrap_err().value_index(), Some(index));
    }
    let mut map = http::HeaderMap::new();
    map.insert("allow", http::HeaderValue::from_static("bad method"));
    assert_eq!(Allow::owned(&map).unwrap_err().kind(), DecodeErrorKind::InvalidToken);
    let mut empty = http::HeaderMap::new();
    empty.append_values(&FieldName::Allow, EncodedValues::new()).unwrap();
    assert!(empty.is_empty());
    let mut recognized = http::HeaderMap::new();
    recognized.insert("allow", http::HeaderValue::from_static("GET, POST"));
    assert_eq!(
        Allow::owned(&recognized)
            .unwrap()
            .unwrap()
            .methods()
            .map(MethodView::as_str)
            .collect::<Vec<_>>(),
        ["GET", "POST"]
    );
}

#[cfg(feature = "serde")]
#[test]
fn gamma_survivor_serde_preflight_limits_win_over_a_malformed_later_value() {
    macro_rules! early {
        ($owned:ty, $wire:expr) => {
            let first = serde_json::to_string(&FieldValue::from_str(&$wire).unwrap()).unwrap();
            let json = format!("[{first},{{}}]");
            let error = serde_json::from_str::<$owned>(&json).unwrap_err();
            assert!(error.to_string().contains("source limit exceeded"), "{error}");
        };
    }
    early!(CacheControlOwned, std::iter::repeat_n("public", 1025).collect::<Vec<_>>().join(","));
    early!(ContentLengthOwned, std::iter::repeat_n("1", 1025).collect::<Vec<_>>().join(","));
    early!(ContentTypeOwned, format!("text/plain{}", ";a=1".repeat(1024)));
    early!(IfMatchOwned, std::iter::repeat_n("\"x\"", 1025).collect::<Vec<_>>().join(","));
    early!(
        SecWebSocketExtensionsOwned,
        std::iter::repeat_n("x", 1025).collect::<Vec<_>>().join(",")
    );
    early!(SecWebSocketExtensionsOwned, format!("x{}", ";p".repeat(1024)));
    early!(SetCookieOwned, "a".repeat(65537));
    for invalid in [
        r"[[1]]",
        r#"[{"bytes":[97],"sensitivity":"NonSensitive"},true]"#,
        r#"{"bytes":[97],"sensitivity":"Sensitive","unknown":[}"#,
    ] {
        assert!(serde_json::from_str::<EncodedValues>(invalid).err().is_some());
    }
}

#[test]
fn gamma_survivor_content_type_large_parameter_offsets_fall_back_without_panicking() {
    for type_length in [1, 65535, 65536] {
        let type_ = "a".repeat(type_length);
        let wire = format!("{type_}/b;p=value;q=\"quoted\"");
        let owned = ContentTypeOwned::try_from(wire.as_str()).unwrap();
        assert_eq!(owned.type_().unwrap(), type_);
        assert_eq!(owned.subtype().unwrap(), "b");
        assert_eq!(owned.parameter("p").unwrap(), Some(b"value".as_slice()));
        assert_eq!(owned.parameter("q").unwrap(), Some(b"\"quoted\"".as_slice()));
        assert_eq!(owned.parameters().map(|item| item.unwrap().name()).collect::<Vec<_>>(), ["p", "q"]);
    }
}

#[test]
fn gamma_survivor_padding_empty_quote_tails_and_opaque_policy_serde_contracts() {
    for padding in [" ", "\t"] {
        for wire in [
            format!("{padding}Sunday, 06-Nov-94 08:49:37 GMT"),
            format!("Sunday, 06-Nov-94 08:49:37 GMT{padding}"),
        ] {
            assert_eq!(
                LastModified::view_with(&source::<LastModified>(&[&wire]), DecodeMode::Strict)
                    .unwrap_err()
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }
    accepts::<Accept>(&["text/plain;x=\"value\",,,"]);
    rejects::<Accept>(&["text/plain;q", "text/plain"]);
    rejects::<AcceptEncoding>(&["gzip;q"]);
    let source = source::<AcceptRanges>(&[" ", "bytes"]);
    assert_eq!(AcceptRanges::view(&source).unwrap().unwrap().units().collect::<Vec<_>>(), ["bytes"]);
    #[cfg(feature = "serde")]
    {
        let policy = ContentSecurityPolicyOwned::from_bytes(b"default-src 'self'").unwrap();
        let roundtrip: ContentSecurityPolicyOwned = serde_json::from_str(&serde_json::to_string(&policy).unwrap()).unwrap();
        assert_eq!(
            roundtrip.policy_strs().map(Result::unwrap).collect::<Vec<_>>(),
            ["default-src 'self'"]
        );
    }
}

#[cfg(feature = "http")]
#[test]
fn gamma_survivor_trusted_oversized_weighted_member_cannot_reenter_a_bounded_builder() {
    let mut source = http::HeaderMap::new();
    source.insert("accept-encoding", http::HeaderValue::from_str(&"a".repeat(65537)).unwrap());
    let header = AcceptEncoding::view(&source).unwrap().unwrap();
    let entry = header.entries().next().unwrap();
    assert_eq!(
        AcceptEncodingOwned::from_entries([entry]).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
}

#[cfg(feature = "http")]
struct BeginOnly;

#[cfg(feature = "http")]
impl FieldEncoder for BeginOnly {
    fn encode<O: FieldEncodeOutput>(self, output: &mut O) -> Result<(), InsertError> {
        let _writer = output.begin_value(isize::MAX as usize, FieldSensitivity::NonSensitive)?;
        Err(InsertError::new(InsertErrorKind::InvalidValue))
    }
}

#[cfg(feature = "http")]
#[test]
fn gamma_survivor_http_announced_maximum_is_checked_without_allocating() {
    let mut sink = http::HeaderMap::new();
    assert_eq!(
        sink.set_encoded(&FieldName::Allow, BeginOnly).unwrap_err().kind(),
        InsertErrorKind::InvalidValue
    );
    assert!(sink.is_empty());
}

#[test]
fn gamma_survivor_negotiation_component_lengths_reject_one_byte_past_every_width() {
    for weight in ["0", "1", "0.1", "0.12", "0.123"] {
        let quality = QualityView::parse(weight.as_bytes(), DecodeMode::Strict).unwrap();
        for extra in [0, 1] {
            let coding = "a".repeat(65536 - 3 - weight.len() + extra);
            let entry = AcceptEncodingEntry::new(ContentCoding::parse(&coding).unwrap(), Some(quality));
            assert_eq!(entry.map(|_| ()).ok(), (extra == 0).then_some(()));
        }
    }
    let range = MediaRange::parse("text/plain").unwrap();
    for extra in [0, 1] {
        let raw = vec![b'a'; 65523 + extra];
        let parameter = NegotiationParameter::new(
            NegotiationToken::new("x").unwrap(),
            Some(NegotiationParameterValue::parse(&raw).unwrap()),
        );
        assert_eq!(
            AcceptEntry::new(range, &[parameter], None, &[]).map(|_| ()).ok(),
            (extra == 0).then_some(())
        );
    }
    let long = format!("0.{}1", "0".repeat(65536));
    let quality = QualityView::parse(long.as_bytes(), DecodeMode::Relaxed).unwrap();
    assert_eq!(
        AcceptLanguageEntry::new(LanguageRange::parse("en").unwrap(), Some(quality))
            .err()
            .unwrap()
            .kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
}
