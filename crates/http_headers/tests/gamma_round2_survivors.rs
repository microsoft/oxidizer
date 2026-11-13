// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Distinguishing cases for the second survivor round.

#![cfg(feature = "headers-all")]

use std::cmp::Ordering;

use http_headers::headers::*;
use http_headers::sink::{EncodedValues, FieldEncodeOutput, FieldEncoder, FieldSink, FieldValueWriter, InsertError, InsertErrorKind};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, FieldValueRef};

use self::common::TestMap;

mod common;

#[test]
fn gamma_round2_accept_ranges_empty_slots_do_not_spend_the_budget() {
    let wire = format!("{}bytes", ",".repeat(1024));
    let source = source::<AcceptRanges>(&[&wire]);
    assert_eq!(AcceptRanges::view(&source).unwrap().unwrap().units().collect::<Vec<_>>(), ["bytes"]);
    assert_eq!(
        AcceptRanges::owned(&source).unwrap().unwrap().units().collect::<Vec<_>>(),
        ["bytes"]
    );
}

#[test]
fn gamma_round2_websocket_protocol_and_version_empty_slots_do_not_spend_budget() {
    let protocols = format!("{}chat", ",".repeat(1024));
    let protocol_source = source::<SecWebSocketProtocol>(&[&protocols]);
    assert!(SecWebSocketProtocol::owned(&protocol_source).unwrap().is_some());
    let extension_source = source::<SecWebSocketExtensions>(&[&protocols]);
    assert!(SecWebSocketExtensions::view(&extension_source).unwrap().is_some());
    assert!(SecWebSocketExtensions::owned(&extension_source).unwrap().is_some());
    let versions = format!("{}13", ",".repeat(1024));
    let version_source = source::<SecWebSocketVersion>(&[&versions]);
    assert!(SecWebSocketVersion::view(&version_source).unwrap().is_some());
}

#[test]
fn gamma_round2_one_character_agent_with_trailing_ows_is_nonempty() {
    let source = source::<UserAgent>(&["x "]);
    assert!(UserAgent::view(&source).unwrap().is_some());
    assert!(UserAgent::owned(&source).unwrap().is_some());
}

fn source<H: Field>(values: &[&str]) -> TestMap {
    let mut result = TestMap::default();
    result.0.insert(
        H::name().clone(),
        values
            .iter()
            .map(|value| FieldValue::from_str(value).expect("fixtures contain valid field-value bytes"))
            .collect(),
    );
    result
}

fn reject<H: Field>(wire: &str) {
    let source = source::<H>(&[wire]);
    for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
        assert!(H::view_with(&source, mode).err().is_some(), "{wire}");
        assert!(H::owned_with(&source, mode).err().is_some(), "{wire}");
    }
}

#[test]
fn gamma_round2_quality_compact_fraction_order_checks_each_decimal_place() {
    for fraction in ["0.1234", "0.2718", "0.9999", "0.0101", "0.0011"] {
        let parsed = QualityView::parse(fraction.as_bytes(), DecodeMode::Relaxed).unwrap();
        let prefix: u16 = fraction[2..5].parse().unwrap();
        for thousandths in [prefix.saturating_sub(1), prefix, prefix + 1] {
            let compact = QualityView::from(Quality::from_thousandths(thousandths).unwrap());
            let expected = if thousandths <= prefix { Ordering::Less } else { Ordering::Greater };
            assert_eq!(compact.cmp(&parsed), expected, "{thousandths}, {fraction}");
            assert_eq!(parsed.cmp(&compact), expected.reverse(), "{thousandths}, {fraction}");
            assert_ne!(compact, parsed);
        }
    }
}

#[test]
fn gamma_round2_string_left_equality_rejects_different_field_values() {
    let value = FieldValue::from_static("actual");
    let view = value.as_field_value_ref();
    assert!(!<str as PartialEq<FieldValue>>::eq("different", &value));
    assert!(!<&str as PartialEq<FieldValue>>::eq(&"different", &value));
    assert!(!<str as PartialEq<FieldValueRef<'_>>>::eq("different", &view));
    assert!(!<&str as PartialEq<FieldValueRef<'_>>>::eq(&"different", &view));
    assert!(!FieldValue::from_owner(bytes::Bytes::from_static(b"owned")).unwrap().is_sensitive());
}

#[test]
fn gamma_round2_location_conversions_mark_every_storage_form_sensitive() {
    for location in [
        LocationOwned::try_from("/relative").unwrap(),
        LocationOwned::try_from(String::from("/relative")).unwrap(),
        LocationOwned::try_from(FieldValue::from_static("/relative")).unwrap(),
    ] {
        let value = location.into_field_value();
        assert_eq!(value.as_bytes(), b"/relative");
        assert!(value.is_sensitive());
    }
}

#[test]
fn gamma_round2_relaxed_content_type_retains_offsets_counts_and_parameter_ows() {
    let source = source::<ContentType>(&["image \t/ \tsvg+xml \t; \tfirst=one ; second=\"two\""]);
    assert_eq!(ContentType::view(&source).unwrap_err().kind(), DecodeErrorKind::InvalidSyntax);
    let view = ContentType::view_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    let owned = ContentType::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
    assert_eq!(view.type_().unwrap(), "image");
    assert_eq!(view.subtype().unwrap(), "svg+xml");
    assert_eq!(owned.type_().unwrap(), "image");
    assert_eq!(owned.subtype().unwrap(), "svg+xml");
    for parameters in [view.parameters(), owned.parameters()] {
        assert_eq!(parameters.size_hint(), (0, Some(2)));
        assert_eq!(
            parameters
                .map(|parameter| {
                    let parameter = parameter.unwrap();
                    (parameter.name().to_owned(), parameter.value().to_vec())
                })
                .collect::<Vec<_>>(),
            [("first".to_owned(), b"one".to_vec()), ("second".to_owned(), b"\"two\"".to_vec())]
        );
    }
    for wire in ["application/json; charset=utf-8", "text/html; charset=utf-8"] {
        let value = ContentTypeOwned::try_from(wire).unwrap();
        let parameters = value.parameters().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(parameters.len(), 1);
        assert_eq!(value.parameter("missing").unwrap(), None);
    }
    let constructed = ContentTypeOwned::new("image", "png").unwrap();
    assert_eq!(constructed.type_().unwrap(), "image");
    assert_eq!(constructed.parameters().size_hint(), (0, Some(0)));
}

#[test]
fn gamma_round2_cors_only_a_later_line_may_contain_the_first_item() {
    let source = source::<AccessControlRequestHeaders>(&["", "X-Later"]);
    assert_eq!(
        AccessControlRequestHeaders::view(&source)
            .unwrap()
            .unwrap()
            .iter()
            .map(FieldNameView::as_str)
            .collect::<Vec<_>>(),
        ["X-Later"]
    );
    assert_eq!(
        AccessControlRequestHeaders::owned(&source)
            .unwrap()
            .unwrap()
            .iter()
            .map(FieldNameView::as_str)
            .collect::<Vec<_>>(),
        ["X-Later"]
    );
    assert_eq!(AccessControlAllowMethodsOwned::try_from(FieldValue::default()).unwrap().len(), 0);
    assert_eq!(AccessControlAllowHeadersOwned::try_from(FieldValue::default()).unwrap().len(), 0);
    assert_eq!(AccessControlExposeHeadersOwned::try_from(FieldValue::default()).unwrap().len(), 0);
    assert_eq!(
        AccessControlRequestHeadersOwned::try_from(FieldValue::default())
            .unwrap_err()
            .kind(),
        DecodeErrorKind::InvalidSyntax
    );
    let units = AcceptRangesOwned::from_units(["items"]).unwrap();
    assert_eq!(units.to_string(), "items");
    assert_eq!(units.units().collect::<Vec<_>>(), ["items"]);
    assert_eq!(AcceptRangesOwned::from_units(["none"]).unwrap().to_string(), "none");
    assert_eq!(
        format!("{:?}", AcceptRangesOwned::from_units(["none"]).unwrap()),
        "AcceptRangesOwned { value_count: 1, none: true }"
    );
    reject::<AcceptRanges>("items, none");
}

#[test]
fn gamma_round2_range_relaxed_empty_slots_and_equal_endpoints_are_preserved() {
    let source = source::<Range>(&["bytes= , 10 - 10, , - 3"]);
    let expected = [ByteRangeSpec::FromTo { first: 10, last: 10 }, ByteRangeSpec::Suffix { length: 3 }];
    assert_eq!(
        Range::view_with(&source, DecodeMode::Relaxed)
            .unwrap()
            .unwrap()
            .byte_ranges()
            .unwrap()
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        Range::owned_with(&source, DecodeMode::Relaxed)
            .unwrap()
            .unwrap()
            .byte_ranges()
            .unwrap()
            .collect::<Vec<_>>(),
        expected
    );
    let built = RangeOwned::bytes([ByteRangeSpec::FromTo { first: 10, last: 10 }]).unwrap();
    assert_eq!(built.byte_ranges().unwrap().collect::<Vec<_>>(), [expected[0]]);
}

#[test]
fn gamma_round2_websocket_sole_escaped_parameter_has_nonzero_decoded_length() {
    for wire in ["x;p=\"\\a\"", "x;p=\"\\a\", y;q=two", "x;p=\"a\\b\""] {
        let source = source::<SecWebSocketExtensions>(&[wire]);
        let owned = SecWebSocketExtensions::owned(&source).unwrap().unwrap();
        let view = SecWebSocketExtensions::view(&source).unwrap().unwrap();
        for extensions in [
            owned.extensions().collect::<Result<Vec<_>, _>>().unwrap(),
            view.extensions().collect::<Result<Vec<_>, _>>().unwrap(),
        ] {
            assert_eq!(extensions.len(), if wire.contains(',') { 2 } else { 1 });
            let parameter = extensions[0].parameters().next().unwrap().unwrap();
            assert_eq!(parameter.name(), "p");
            assert!(parameter.is_quoted());
            assert_eq!(
                parameter.value().unwrap(),
                wire.split_once('=').unwrap().1.split(',').next().unwrap().as_bytes()
            );
        }
    }
    for builder in [
        SecWebSocketExtensionsOwned::builder().quoted_parameter("p", "a"),
        SecWebSocketExtensionsOwned::builder()
            .extension("x")
            .quoted_parameter("bad name", "a"),
    ] {
        assert_eq!(builder.build().unwrap_err().kind(), DecodeErrorKind::InvalidToken);
    }
    reject::<SecWebSocketExtensions>("x;p=\"\"");
}

#[test]
fn gamma_round2_forwarded_source_contains_uses_the_underlying_override() {
    struct ContainsOnly;
    impl FieldSource for ContainsOnly {
        fn lines(&self, _name: &'static FieldName) -> Option<FieldLines<'_>> {
            None
        }
        fn contains(&self, name: &'static FieldName) -> bool {
            name == &FieldName::Vary
        }
    }
    assert!(<&ContainsOnly as FieldSource>::contains(&&ContainsOnly, &FieldName::Vary));
    assert!(!<&ContainsOnly as FieldSource>::contains(&&ContainsOnly, &FieldName::Allow));
}

#[test]
fn gamma_round2_deferred_sensitive_writes_replace_existing_values() {
    let auth_source = source::<Authorization<Bearer>>(&["Bearer token"]);
    let cookies_source = source::<SetCookie>(&["a=b", "c=d"]);
    let mut sink = TestMap::default();
    sink.set_values(&FieldName::Authorization, EncodedValues::single(FieldValue::from_static("old")))
        .unwrap();
    sink.set_values(&FieldName::SetCookie, EncodedValues::single(FieldValue::from_static("old")))
        .unwrap();
    Authorization::<Bearer>::view(&auth_source)
        .unwrap()
        .unwrap()
        .insert_into(&mut sink)
        .unwrap();
    SetCookie::view(&cookies_source).unwrap().unwrap().insert_into(&mut sink).unwrap();
    for (name, expected) in [
        (&FieldName::Authorization, vec![b"Bearer token".as_slice()]),
        (&FieldName::SetCookie, vec![b"a=b".as_slice(), b"c=d"]),
    ] {
        let values = sink.get_all(name);
        assert_eq!(values.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(), expected);
        assert!(values.iter().all(FieldValue::is_sensitive));
    }
}

#[cfg(feature = "http")]
#[test]
fn gamma_round2_borrowed_http_storage_preserves_both_sensitivity_states() {
    for length in [1, 64, 65, 128] {
        for sensitive in [false, true] {
            let mut http = http::HeaderValue::from_bytes(&vec![b'a'; length]).unwrap();
            http.set_sensitive(sensitive);
            let value = FieldValue::from(&http);
            assert_eq!(value.is_sensitive(), sensitive);
            assert_eq!(value.as_bytes(), http.as_bytes());
            assert_eq!(http::HeaderValue::try_from(value).unwrap().is_sensitive(), sensitive);
        }
    }
}

#[test]
fn gamma_round2_list_limits_count_only_unquoted_members_and_accept_exactly_the_budget() {
    let wire = format!("{}1", ",".repeat(1024));
    let length_source = source::<ContentLength>(&[&wire]);
    assert_eq!(
        ContentLength::view(&length_source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    let wire = format!("max-age=1{}", ";".repeat(1024));
    let source = source::<StrictTransportSecurity>(&[&wire]);
    assert_eq!(
        StrictTransportSecurity::view(&source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
}

#[test]
fn gamma_round2_trailing_escape_reports_the_correct_field_line_index() {
    let values = [FieldValue::from_static("first"), FieldValue::from_static("\"unfinished\\")];
    let lines = FieldLines::from_slice(&FieldName::Vary, &values).unwrap();
    let mut items = lines.comma_items();
    assert_eq!(items.next().unwrap().unwrap(), b"first");
    let error = items.next().unwrap().unwrap_err();
    assert_eq!(error.kind(), DecodeErrorKind::UnterminatedQuote);
    assert_eq!(error.value_index(), Some(1));
}

#[test]
fn gamma_round2_origin_constructor_preserves_long_domain_without_a_port() {
    let domain = format!("{}.{}.{}.{}.", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61));
    let domain = OriginDomainView::new(&domain).unwrap();
    let origin = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, OriginHost::Domain(domain), None).unwrap();
    assert_eq!(origin.as_str().unwrap(), format!("https://{}", domain.as_str()));
    assert_eq!(origin.to_string(), format!("https://{}", domain.as_str()));
    let AccessControlAllowOriginKind::Origin(parts) = origin.kind() else {
        panic!("constructed tuple origin");
    };
    assert_eq!(parts.scheme(), OriginScheme::Https);
    assert_eq!(parts.host(), OriginHost::Domain(domain));
    assert_eq!(parts.port(), None);
}

#[test]
fn gamma_round2_host_percent_triplets_cannot_skip_a_later_forbidden_byte() {
    for wire in ["abc%20@", "abc%20/", "abc%20[", "abc%20:bad", "abc%20%xx"] {
        reject::<Host>(wire);
    }
}

#[test]
fn gamma_round2_owned_negotiation_debug_reports_every_retained_field_line() {
    let source = source::<AcceptEncoding>(&["gzip", "deflate", "br"]);
    let owned = AcceptEncoding::owned(&source).unwrap().unwrap();
    assert_eq!(format!("{owned:?}"), "AcceptEncodingOwned { value_count: 3 }");
}

#[test]
fn gamma_round2_nosniff_encoding_keeps_the_decoded_spelling() {
    let input = FieldValue::from_static("nosniff").with_sensitivity(http_headers::FieldSensitivity::Sensitive);
    let owned = <XContentTypeOptions as http_headers::SingleValueField>::decode_owned(input).unwrap();
    let value = <XContentTypeOptions as http_headers::SingleValueField>::into_field_value(owned);
    assert_eq!(value.as_bytes(), b"nosniff");
    assert!(value.is_sensitive());
}

#[test]
fn gamma_round2_range_comma_without_ows_preserves_the_next_first_digit() {
    let source = source::<Range>(&["bytes=1-2,34-56,78-"]);
    assert_eq!(
        Range::owned(&source).unwrap().unwrap().byte_ranges().unwrap().collect::<Vec<_>>(),
        [
            ByteRangeSpec::FromTo { first: 1, last: 2 },
            ByteRangeSpec::FromTo { first: 34, last: 56 },
            ByteRangeSpec::From { first: 78 },
        ]
    );
}

#[test]
fn gamma_round2_accept_ranges_casefolds_none_and_preserves_canonical_constructors() {
    for none in ["NONE", "NoNe", "nONe"] {
        let owned = AcceptRangesOwned::from_units([none]).unwrap();
        assert!(owned.is_none());
        assert_eq!(owned.units().collect::<Vec<_>>(), [none]);
        reject::<AcceptRanges>(&format!("items,{none}"));
    }
    for units in [vec!["bytes"], vec!["items"], vec!["bytes", "items"]] {
        let owned = AcceptRangesOwned::from_units(&units).unwrap();
        assert!(!owned.is_none());
        assert_eq!(owned.units().collect::<Vec<_>>(), units);
        assert_eq!(owned.to_string(), units.join(", "));
    }
    for wire in ["items,,", ",items,", "none,,"] {
        let source = source::<AcceptRanges>(&[wire, ""]);
        let view = AcceptRanges::view(&source).unwrap().unwrap();
        assert_eq!(view.is_none(), wire.starts_with("none"));
    }
}

#[test]
fn gamma_round2_invalid_sources_report_precise_indices_and_etag_decode_returns_error() {
    let source = source::<ContentLength>(&["invalid"]);
    assert_eq!(ContentLength::view(&source).unwrap_err().value_index(), Some(0));
    for bytes in [b"invalid".as_slice(), b"\"a b\"", b"W\"tag\""] {
        assert_eq!(
            <ETag as http_headers::SingleValueField>::decode_view(FieldValueRef::new(bytes))
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}

#[test]
fn gamma_round2_location_construction_checks_relative_colons_and_ipvfuture_versions() {
    assert!(LocationOwned::from_components(None, None, "x:y", None, None).err().is_some());
    assert!(LocationOwned::from_components(None, None, "x:y/", None, None).err().is_some());
    assert_eq!(
        LocationOwned::from_components(Some("foo"), None, "x:y/", None, None)
            .unwrap()
            .as_bytes(),
        b"foo:x:y/"
    );
    assert!(UriAuthority::new(None, "[v1G.address]", None).err().is_some());
}

#[test]
fn gamma_round2_hsts_extensions_validate_reserved_names_and_invalid_values() {
    for name in ["max-age", "preload", "MAX-AGE", "PRELOAD"] {
        assert!(
            StrictTransportSecurityOwned::builder(std::time::Duration::from_secs(1))
                .extension_flag(name)
                .build()
                .err()
                .is_some()
        );
    }
    assert!(
        StrictTransportSecurityOwned::builder(std::time::Duration::from_secs(1))
            .extension_value("x", "bad value")
            .build()
            .err()
            .is_some()
    );
    reject::<StrictTransportSecurity>("unknown=1");
    let source = source::<StrictTransportSecurity>(&["unknown=999; max-age=7"]);
    assert_eq!(StrictTransportSecurity::owned(&source).unwrap().unwrap().max_age().as_secs(), 7);
}

#[test]
fn gamma_round2_encoded_values_push_keeps_vec_backed_values_before_the_new_one() {
    let mut values = EncodedValues::from_vec(vec![FieldValue::from_static("first"), FieldValue::from_static("second")]);
    values.push(FieldValue::from_static("third"));
    assert_eq!(
        values.iter().map(FieldValue::as_bytes).collect::<Vec<_>>(),
        [b"first".as_slice(), b"second", b"third"]
    );
}

struct OverlongWrite;

impl FieldEncoder for OverlongWrite {
    fn encode<O: FieldEncodeOutput>(self, output: &mut O) -> Result<(), InsertError> {
        let mut writer = output.begin_value(1, http_headers::FieldSensitivity::NonSensitive)?;
        assert_eq!(
            writer.write_bytes(b"xx").expect_err("chunk exceeds the announced length").kind(),
            InsertErrorKind::InvalidEncoding
        );
        Err(InsertError::new(InsertErrorKind::InvalidEncoding))
    }
}

#[test]
fn gamma_round2_writer_rejects_an_overlong_chunk_before_finish() {
    let mut sink = TestMap::default();
    assert_eq!(
        sink.set_encoded(&FieldName::Vary, OverlongWrite).unwrap_err().kind(),
        InsertErrorKind::InvalidEncoding
    );
}

#[cfg(feature = "serde")]
#[test]
fn gamma_round2_serde_malformed_keys_sequences_and_field_name_are_errors_not_panics() {
    for json in [
        r#"{"bytes":[],"sensitivity":"NonSensitive", 1:0}"#,
        r#"{"bytes":[],"sensitivity":"NonSensitive","#,
        r"[[97], tru]",
    ] {
        assert!(serde_json::from_str::<FieldValue>(json).err().is_some(), "{json}");
    }
    assert!(serde_json::from_str::<FieldName>("true").err().is_some());
}

#[test]
fn gamma_round2_host_parts_preserve_ipvfuture_version_and_address_offsets() {
    for wire in ["[v1.a]", "[v123.a:b]", "[vABCD.a-b]"] {
        let parsed = HostOwned::try_from(wire).unwrap();
        let constructed = HostOwned::from_parts(parsed.kind(), None).unwrap();
        assert_eq!(constructed.as_field_value().as_bytes(), wire.as_bytes());
        assert_eq!(constructed.kind(), parsed.kind());
    }
}

#[test]
fn gamma_round2_negotiation_builders_enforce_exact_separator_budget() {
    let first = "a".repeat(32_767);
    for (second_length, succeeds) in [(32_767, true), (32_768, false)] {
        let second = "b".repeat(second_length);
        let entries = [&first, &second]
            .map(|token| AcceptEncodingEntry::new(ContentCoding::new(NegotiationToken::new(token).unwrap()), None).unwrap());
        let result = AcceptEncodingOwned::from_entries(entries);
        if succeeds {
            let value = result.unwrap();
            assert_eq!(value.items().collect::<Vec<_>>(), [first.as_bytes(), second.as_bytes()]);
            assert_eq!(
                value.entries().map(|entry| entry.coding().as_str()).collect::<Vec<_>>(),
                [first.as_str(), second.as_str()]
            );
        } else {
            assert_eq!(result.unwrap_err().kind(), DecodeErrorKind::SourceLimitExceeded);
        }
    }
}

#[test]
fn gamma_round2_negotiation_entries_retain_media_parameters_quality_and_extensions() {
    let accept_source = source::<Accept>(&["text/plain; charset=utf-8; q=0.5; level=2; flag"]);
    let owned = Accept::owned(&accept_source).unwrap().unwrap();
    let entry = owned.entries().next().unwrap();
    assert_eq!(
        entry
            .parameters()
            .map(|parameter| (
                parameter.name().as_str(),
                parameter.value().map(NegotiationParameterValue::raw_bytes)
            ))
            .collect::<Vec<_>>(),
        [("charset", Some(b"utf-8".as_slice()))]
    );
    assert_eq!(entry.quality(), QualityView::from(Quality::from_thousandths(500).unwrap()));
    assert_eq!(
        entry
            .extensions()
            .map(|parameter| (
                parameter.name().as_str(),
                parameter.value().map(NegotiationParameterValue::raw_bytes)
            ))
            .collect::<Vec<_>>(),
        [("level", Some(b"2".as_slice())), ("flag", None)]
    );
    for wire in ["gzip;q=0.5", "gzip; q = .5", "gzip;q= .5"] {
        let source = source::<AcceptEncoding>(&[wire]);
        let owned = AcceptEncoding::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
        let entry = owned.entries().next().unwrap();
        assert_eq!(entry.coding().as_str(), "gzip");
        assert_eq!(entry.quality(), QualityView::from(Quality::from_thousandths(500).unwrap()));
    }
}

#[test]
fn gamma_round2_validation_errors_are_not_replaced_with_panics_or_lost_indices() {
    for wire in ["x=bad value", "max-age=\"unterminated"] {
        assert!(CacheControlOwned::try_from(wire.to_owned()).err().is_some());
        assert!(CacheControlOwned::try_from(FieldValue::from_str(wire).unwrap()).err().is_some());
    }
    assert!(
        CacheControlOwned::builder()
            .extension_value("x", "bad value")
            .build()
            .err()
            .is_some()
    );
    assert!(CacheControlOwned::builder().extension_value("x", "a,b").build().err().is_some());
    assert!(
        AccessControlAllowHeadersOwned::try_from(FieldValue::from_static("bad/name"))
            .err()
            .is_some()
    );
    let cors_source = source::<AccessControlRequestHeaders>(&["bad/name", "good"]);
    assert_eq!(AccessControlRequestHeaders::view(&cors_source).unwrap_err().value_index(), Some(0));
    for source in [
        source::<Accept>(&["text/plain", "bad"]),
        source::<Accept>(&["text/plain", "x/y;q=2"]),
    ] {
        assert_eq!(Accept::view(&source).unwrap_err().value_index(), None);
        assert_eq!(Accept::owned(&source).unwrap_err().value_index(), None);
    }
    let source = source::<AcceptEncoding>(&["gzip", "bad/name"]);
    assert_eq!(AcceptEncoding::view(&source).unwrap_err().value_index(), None);
    assert_eq!(
        VaryOwned::try_from(FieldValue::from_static("bad/name")).unwrap_err().value_index(),
        None
    );
}

struct PushedValue;

impl FieldEncoder for PushedValue {
    fn encode<O: FieldEncodeOutput>(self, output: &mut O) -> Result<(), InsertError> {
        output.push_value(FieldValue::from_static("retained"))
    }
}

#[test]
fn gamma_round2_encoder_push_value_retains_nonempty_payload() {
    let mut sink = TestMap::default();
    sink.set_encoded(&FieldName::Vary, PushedValue).unwrap();
    assert_eq!(
        sink.lines(&FieldName::Vary).unwrap().repeated().next().unwrap().as_bytes(),
        b"retained"
    );
}

#[cfg(feature = "serde")]
#[test]
fn gamma_round2_serde_budget_end_still_propagates_malformed_trailing_elements() {
    let field = r#"{"bytes":[97],"sensitivity":"NonSensitive"}"#;
    let line_overflow = format!("[{},tru]", vec![field; 128].join(","));
    assert!(serde_json::from_str::<SetCookieOwned>(&line_overflow).err().is_some());
    let byte_overflow = format!(
        r#"[{{"bytes":[{},tru],"sensitivity":"NonSensitive"}}]"#,
        vec!["97"; 65_536].join(","),
    );
    assert!(serde_json::from_str::<SetCookieOwned>(&byte_overflow).err().is_some());
    assert!(
        serde_json::from_str::<SetCookieOwned>(r#"[{"bytes":[],"sensitivity":"NonSensitive"}]"#)
            .err()
            .is_some()
    );
}

#[test]
fn gamma_round2_cors_and_websocket_list_budgets_ignore_only_comma_empty_members() {
    let invalid_and_excessive = format!("bad/name,{}", vec!["x"; 1024].join(","));
    let excessive_source = source::<AccessControlRequestHeaders>(&[&invalid_and_excessive]);
    assert_eq!(
        AccessControlRequestHeaders::view(&excessive_source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    assert_eq!(
        AccessControlRequestHeaders::owned(&excessive_source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    for (wire, accepted) in [(format!("{}x", ",".repeat(1024)), true), (vec!["x"; 1025].join(","), false)] {
        let source = source::<AccessControlRequestHeaders>(&[&wire]);
        for result in [
            AccessControlRequestHeaders::view(&source).map(|value| value.is_some()),
            AccessControlRequestHeaders::owned(&source).map(|value| value.is_some()),
        ] {
            if accepted {
                assert!(result.unwrap());
            } else {
                assert_eq!(result.unwrap_err().kind(), DecodeErrorKind::SourceLimitExceeded);
            }
        }
    }
    let semicolons = format!("x{}", ";".repeat(1024));
    let source = source::<SecWebSocketExtensions>(&[&semicolons]);
    assert_eq!(
        SecWebSocketExtensions::view(&source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
    assert_eq!(
        SecWebSocketExtensions::owned(&source).unwrap_err().kind(),
        DecodeErrorKind::SourceLimitExceeded
    );
}

#[test]
fn gamma_round2_relaxed_content_range_rejects_leading_payload_ows() {
    for wire in ["Bytes  0-1/2", "Bytes \t0-1/2", "Bytes  0-1/2 "] {
        let source = source::<ContentRange>(&[wire]);
        assert_eq!(
            ContentRange::view_with(&source, DecodeMode::Relaxed).unwrap_err().kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }
}

#[test]
fn gamma_round2_bare_accept_extension_counts_no_unwritten_equals_byte() {
    let name = "x".repeat(65_528);
    let extensions = [NegotiationParameter::new(NegotiationToken::new(&name).unwrap(), None)];
    let entry = AcceptEntry::new(MediaRange::parse("x/y").unwrap(), &[], Some(QualityView::ONE), &extensions).unwrap();
    let owned = AcceptOwned::from_entries([entry]).unwrap();
    let source = source::<Accept>(&["x/y"]);
    assert!(Accept::view(&source).unwrap().is_some());
    assert_eq!(owned.entries().next().unwrap().extensions().next().unwrap().name().as_str(), name);
}

#[test]
fn gamma_round2_websocket_quoted_parameter_projection_and_error_kinds_are_exact() {
    let owned = SecWebSocketExtensionsOwned::try_from("x;p=\"a\"").unwrap();
    let extension = owned.extensions().next().unwrap().unwrap();
    let parameter = extension.parameters().next().unwrap().unwrap();
    assert_eq!(parameter.as_bytes(), b"p=\"a\"");
    assert_eq!(parameter.value(), Some(b"\"a\"".as_slice()));
    assert!(parameter.is_quoted());
    assert_eq!(
        SecWebSocketExtensionsOwned::builder()
            .quoted_parameter("a", "b")
            .build()
            .unwrap_err()
            .kind(),
        DecodeErrorKind::InvalidToken
    );
    assert_eq!(
        SecWebSocketExtensionsOwned::builder()
            .extension("x")
            .quoted_parameter("bad/name", "a")
            .build()
            .unwrap_err()
            .kind(),
        DecodeErrorKind::InvalidToken
    );
    assert_eq!(
        SecWebSocketExtensionsOwned::builder()
            .extension("x")
            .quoted_parameter("a", "bad value")
            .build()
            .unwrap_err()
            .kind(),
        DecodeErrorKind::InvalidToken
    );
}
