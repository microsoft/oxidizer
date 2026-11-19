// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error propagation and cached metadata boundaries from the afternoon campaign.

#![cfg(feature = "headers-all")]

use http_headers::headers::*;
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName};

struct ObservedValues<'a> {
    values: std::vec::IntoIter<http_headers::FieldValue>,
    hints: &'a std::cell::RefCell<Vec<usize>>,
    consumed: usize,
}

impl Iterator for ObservedValues<'_> {
    type Item = http_headers::FieldValue;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.values.next()?;
        self.consumed += 1;
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.hints.borrow_mut().push(self.consumed);
        self.values.size_hint()
    }
}

#[test]
fn afternoon_encoded_values_consume_the_inline_first_item_before_reserving_the_remainder() {
    let hints = std::cell::RefCell::new(Vec::new());
    let iterator = ObservedValues {
        values: ["first", "second", "third"]
            .map(http_headers::FieldValue::from_static)
            .to_vec()
            .into_iter(),
        hints: &hints,
        consumed: 0,
    };
    let mut values = http_headers::sink::EncodedValues::new();
    values.extend(iterator);
    assert_eq!(hints.borrow().first(), Some(&1));
    assert_eq!(
        values.iter().map(http_headers::FieldValue::as_bytes).collect::<Vec<_>>(),
        [b"first".as_slice(), b"second", b"third"]
    );
}

struct RawLine<'a> {
    name: &'static FieldName,
    bytes: &'a [u8],
}

impl FieldSource for RawLine<'_> {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::single(name, self.bytes))
    }
}

fn rejected<H: Field>(wires: &[&str]) {
    for wire in wires {
        let source = RawLine {
            name: H::name(),
            bytes: wire.as_bytes(),
        };
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            let view = H::view_with(&source, mode)
                .err()
                .expect("the fixture violates this header's grammar");
            let owned = H::owned_with(&source, mode)
                .err()
                .expect("owned decoding must propagate the same malformed-input failure");
            assert_eq!(view.header(), H::name(), "{wire:?}/{mode:?}");
            assert_eq!(owned.header(), H::name(), "{wire:?}/{mode:?}");
            assert_eq!(view.kind(), owned.kind(), "{wire:?}/{mode:?}");
            #[cfg(feature = "http")]
            {
                let mut trusted = http::HeaderMap::new();
                trusted.insert(
                    http::header::HeaderName::from_bytes(H::name().as_str().as_bytes())
                        .expect("typed header names satisfy the HTTP name grammar"),
                    http::HeaderValue::from_str(wire).expect("malformed grammar fixtures still contain legal field-value bytes"),
                );
                let view = H::view_with(&trusted, mode)
                    .err()
                    .expect("trusted storage does not make malformed header grammar valid");
                let owned = H::owned_with(&trusted, mode)
                    .err()
                    .expect("trusted owned decoding must propagate malformed header grammar");
                assert_eq!(view.header(), H::name());
                assert_eq!(owned.header(), H::name());
                assert_eq!(view.kind(), owned.kind(), "{wire:?}/{mode:?}");
                if matches!(
                    *H::name(),
                    FieldName::Accept | FieldName::AcceptEncoding | FieldName::AcceptLanguage
                ) && owned.kind() != DecodeErrorKind::UnterminatedQuote
                {
                    assert_eq!(owned.value_index(), None, "{wire:?}/{mode:?}");
                }
            }
        }
    }
}

#[test]
fn afternoon_malformed_negotiation_and_media_parameters_propagate_errors() {
    rejected::<Accept>(&[
        "text",
        "/plain",
        "text/",
        "text/plain;p",
        "text/plain;",
        "text/plain;;q=1",
        "text/plain;p=",
        "text/plain;p=\"unterminated",
        "text/plain;q=2",
        "text/plain;q=1.001",
    ]);
    rejected::<AcceptEncoding>(&[
        "gzip;;q=1",
        "gzip;q",
        "gzip;q=",
        "gzip;q=2",
        "gzip;q=\"1\"",
        "gzip;q=\"unterminated",
        "gzip;q=0.5;extra=1",
        "gzip;q=0.5;",
    ]);
    rejected::<AcceptLanguage>(&[
        "en;;q=1",
        "en;q",
        "en;q=",
        "en;q=2",
        "en;q=\"1\"",
        "en;q=\"unterminated",
        "en;q=0.5;extra=1",
    ]);
    rejected::<ContentType>(&[
        "",
        "text",
        "text/",
        "/plain",
        "text/plain;name",
        "text/plain;name=",
        "text/plain;=value",
        "text/plain;name=\"unterminated",
        "text/plain;name=\"x\"junk",
    ]);
    rejected::<CacheControl>(&[
        "max-age=",
        "max-age=\"unterminated",
        "max-age=\"x\"junk",
        "custom=",
        "custom=\"x\"junk",
        "custom=bad value",
    ]);
}

#[test]
fn afternoon_custom_field_names_are_lowercase_across_normalization_storage_boundaries() {
    for wire in ["FRESHNAME".to_owned(), "x-fresh-name".to_owned(), "X".repeat(64), "X".repeat(65)] {
        let name = FieldName::try_from_bytes(&wire).unwrap();
        assert_eq!(name.as_str(), wire.to_ascii_lowercase());
        assert_eq!(name.as_bytes(), wire.to_ascii_lowercase().as_bytes());
    }
}

#[test]
fn afternoon_registered_request_methods_keep_their_canonical_storage_sensitivity_policy() {
    for (wire, retained_sensitive) in [("GET", false), ("POST", false), ("CUSTOM", true)] {
        let mut value = http_headers::FieldValue::from_str(wire).unwrap();
        value.set_sensitivity(http_headers::FieldSensitivity::Sensitive);
        let owned = AccessControlRequestMethodOwned::try_from(value).unwrap();
        let encoded = http_headers::FieldValue::from(owned);
        assert_eq!(encoded.as_bytes(), wire.as_bytes());
        assert_eq!(encoded.is_sensitive(), retained_sensitive);
        #[cfg(feature = "http")]
        {
            let mut source = http::HeaderMap::new();
            let mut value = http::HeaderValue::from_str(wire).unwrap();
            value.set_sensitive(true);
            source.insert(http::header::ACCESS_CONTROL_REQUEST_METHOD, value);
            let owned = AccessControlRequestMethod::owned(&source).unwrap().unwrap();
            let encoded = http_headers::FieldValue::from(owned);
            assert_eq!(encoded.as_bytes(), wire.as_bytes());
            assert_eq!(encoded.is_sensitive(), retained_sensitive);
        }
    }
}

#[test]
fn afternoon_international_hosts_reject_malformed_authority_boundaries_before_normalizing() {
    for wire in ["[ｖ１.abc]", "[é", "é@", "é:1x", "é:x1", "é:x", "é:1:2", "\u{ad}[::1]:80"] {
        let source = RawLine {
            name: &FieldName::Host,
            bytes: wire.as_bytes(),
        };
        for error in [
            Host::view_with(&source, DecodeMode::Relaxed).unwrap_err(),
            Host::owned_with(&source, DecodeMode::Relaxed).unwrap_err(),
        ] {
            assert_eq!(error.header(), &FieldName::Host, "{wire}");
            assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax, "{wire}");
        }
    }
    for (wire, expected_port) in [("é", None), ("é:123", Some("123")), ("é:", Some(""))] {
        let source = RawLine {
            name: &FieldName::Host,
            bytes: wire.as_bytes(),
        };
        let owned = Host::owned_with(&source, DecodeMode::Relaxed).unwrap().unwrap();
        assert_eq!(owned.host().unwrap(), "é", "{wire}");
        assert_eq!(owned.port().unwrap(), expected_port, "{wire}");
        assert_eq!(owned.as_str().unwrap(), wire);
    }
}

#[test]
fn afternoon_quoted_websocket_lists_preserve_presence_before_and_after_the_last_comma() {
    for wire in ["extension;p=\"a\"", "extension;p=\"a\","] {
        let source = RawLine {
            name: &FieldName::SecWebSocketExtensions,
            bytes: wire.as_bytes(),
        };
        let view = SecWebSocketExtensions::view(&source).unwrap().unwrap();
        let owned = SecWebSocketExtensions::owned(&source).unwrap().unwrap();
        assert_eq!(view.extensions().count(), 1, "{wire}");
        assert_eq!(owned.extensions().count(), 1, "{wire}");
        assert_eq!(owned.to_string(), wire);
    }
}

#[test]
fn afternoon_compact_quality_decimal_digits_keep_each_distinct_place_value() {
    for (thousandths, expected) in [
        (0, "0"),
        (1, "0.001"),
        (10, "0.01"),
        (99, "0.099"),
        (123, "0.123"),
        (250, "0.25"),
        (999, "0.999"),
        (1000, "1"),
    ] {
        let quality = Quality::from_thousandths(thousandths).unwrap();
        let view = QualityView::from(quality);
        assert_eq!(quality.to_string(), expected);
        assert_eq!(view.to_string(), expected);
        assert_eq!(QualityView::parse(expected.as_bytes(), DecodeMode::Strict).unwrap(), view);
        assert_eq!(view.to_quality().unwrap().thousandths(), thousandths);
    }
}

#[test]
fn afternoon_range_source_budgets_ignore_empty_comma_slots() {
    let wire = format!("bytes=0-1{}", ",".repeat(http_headers::source::MAX_CUSTOM_LIST_ITEMS));
    let source = RawLine {
        name: &FieldName::Range,
        bytes: wire.as_bytes(),
    };
    assert!(Range::view(&source).unwrap().is_some());
    assert!(Range::owned(&source).unwrap().is_some());
}

#[test]
fn afternoon_content_type_empty_parameter_queries_do_not_read_uninitialized_cache_slots() {
    for wire in [
        "application/json",
        "application/octet-stream",
        "text/html",
        "text/plain",
        "text/css",
        "application/javascript",
        "application/json; charset=utf-8",
        "text/html; charset=utf-8",
        "text / plain",
        "application/custom;p0=v0;p1=v1;p2=v2;p3=v3",
    ] {
        let source = RawLine {
            name: ContentType::name(),
            bytes: wire.as_bytes(),
        };
        let mode = if wire.contains(" / ") {
            DecodeMode::Relaxed
        } else {
            DecodeMode::Strict
        };
        let view = ContentType::view_with(&source, mode).unwrap().unwrap();
        let owned = ContentType::owned_with(&source, mode).unwrap().unwrap();
        assert_eq!(view.parameter("").unwrap(), None, "{wire}");
        assert_eq!(owned.parameter("").unwrap(), None, "{wire}");
        assert_eq!(view.parameter("missing").unwrap(), None, "{wire}");
        assert_eq!(owned.parameter("missing").unwrap(), None, "{wire}");
        let expected = wire.contains("charset=").then_some(b"utf-8".as_slice());
        assert_eq!(view.parameter("CHARSET").unwrap(), expected);
        assert_eq!(owned.parameter("CHARSET").unwrap(), expected);
        assert_eq!(view.parameters().count(), wire.bytes().filter(|byte| *byte == b';').count());
        assert_eq!(owned.parameters().count(), wire.bytes().filter(|byte| *byte == b';').count());
    }
    let constructed = ContentTypeOwned::new("text", "plain").unwrap();
    assert_eq!(constructed.parameter("").unwrap(), None);
    assert_eq!(constructed.parameter("missing").unwrap(), None);
    assert_eq!(constructed.parameters().count(), 0);
    let error = CacheControlOwned::builder().build().unwrap_err();
    assert_eq!(error.header(), &FieldName::CacheControl);
    assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
}

#[test]
fn afternoon_relaxed_date_names_must_have_exact_lengths_before_copying() {
    rejected::<LastModified>(&[
        "Su, 6 Nov 1994 8:49:37 UTC",
        "Sund, 6 Nov 1994 8:49:37 UTC",
        "Sun, 6 No 1994 8:49:37 UTC",
        "Sun, 6 Nove 1994 8:49:37 UTC",
        "Sun, 6 Nov 1994 8:49:37 GMTX",
        "Sun, 6 Nov 1994 :49:37 UTC",
    ]);
    for wire in [
        "Sun, 06 Nov 1994 24:49:37 GMT",
        "Sun, 06 Nov 1994 08:60:37 GMT",
        "Sun, 06 Nov 1994 08:49:60 GMT",
        "Sun, 06 Nov 10000 08:49:37 GMT",
        "Thu, 06 Nov 1969 08:49:37 GMT",
        "Mon, 01 Jan 0001 00:00:00 GMT",
        "Sat, 01 Jan 0000 00:00:00 GMT",
    ] {
        httpdate::parse_http_date(wire).expect_err("the normalization target independently rejects invalid clock and year ranges");
    }
}

#[test]
fn afternoon_owned_singleton_decoders_return_cardinality_errors_instead_of_panicking() {
    use http_headers::FieldValue;

    use self::common::TestMap;

    macro_rules! singleton {
        ($header:ty, $wire:literal) => {{
            let mut source = TestMap::default();
            source.0.insert(<$header>::name().clone(), vec![FieldValue::from_static($wire); 2]);
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                let error = <$header>::owned_with(&source, mode).unwrap_err();
                assert_eq!(error.header(), <$header>::name());
                assert_eq!(error.kind(), DecodeErrorKind::UnexpectedMultipleValues);
            }
        }};
    }
    singleton!(AccessControlRequestMethod, "GET");
    singleton!(ContentType, "text/plain");
    singleton!(Host, "example.com");
    singleton!(Location, "/");
}

#[test]
fn afternoon_custom_field_sources_cannot_bypass_owned_decoder_admission() {
    fn invalid_bytes<H: Field>() {
        for bytes in [b"\r".as_slice(), b"\n", b"\0", b"\x7f", b"value\r\ninjected: value"] {
            let source = RawLine { name: H::name(), bytes };
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                let view = H::view_with(&source, mode)
                    .err()
                    .expect("custom field bytes violate the admission invariant");
                let owned = H::owned_with(&source, mode)
                    .err()
                    .expect("owned decoding must propagate custom-source rejection rather than panic");
                assert_eq!(view.header(), H::name());
                assert_eq!(owned.header(), H::name());
                assert_eq!(view.kind(), DecodeErrorKind::InvalidSyntax);
                assert_eq!(owned.kind(), DecodeErrorKind::InvalidSyntax);
            }
        }
    }

    invalid_bytes::<Authorization<Basic>>();
    invalid_bytes::<Authorization<Bearer>>();
    invalid_bytes::<CacheControl>();
    invalid_bytes::<ContentType>();
    invalid_bytes::<ContentLength>();
    invalid_bytes::<ETag>();
    invalid_bytes::<LastModified>();
    invalid_bytes::<Location>();
    invalid_bytes::<Host>();
    invalid_bytes::<Accept>();
    invalid_bytes::<AcceptEncoding>();
    invalid_bytes::<AcceptLanguage>();
    invalid_bytes::<AccessControlAllowOrigin>();
    invalid_bytes::<AccessControlRequestMethod>();
    invalid_bytes::<AccessControlAllowCredentials>();
    invalid_bytes::<AccessControlMaxAge>();
    invalid_bytes::<AccessControlAllowMethods>();
    invalid_bytes::<AccessControlAllowHeaders>();
    invalid_bytes::<AccessControlExposeHeaders>();
    invalid_bytes::<AccessControlRequestHeaders>();
    invalid_bytes::<Range>();
    invalid_bytes::<ContentRange>();
    invalid_bytes::<AcceptRanges>();
    invalid_bytes::<ReferrerPolicy>();
    invalid_bytes::<ContentSecurityPolicy>();
    invalid_bytes::<StrictTransportSecurity>();
    invalid_bytes::<SetCookie>();
    invalid_bytes::<SecWebSocketExtensions>();
    invalid_bytes::<SecWebSocketProtocol>();
    invalid_bytes::<SecWebSocketVersion>();
    invalid_bytes::<SecWebSocketKey>();
    invalid_bytes::<SecWebSocketAccept>();
}

mod common;

#[test]
fn afternoon_builders_reject_pending_errors_and_directive_injection() {
    let error = AccessControlAllowMethodsOwned::from_methods([""]).unwrap_err();
    assert_eq!(error.header(), &FieldName::AccessControlAllowMethods);
    assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
    for value in ["a; preload", "a; includeSubDomains"] {
        let error = StrictTransportSecurityOwned::builder(std::time::Duration::from_secs(17))
            .extension_value("custom", value)
            .build()
            .unwrap_err();
        assert_eq!(error.header(), &FieldName::StrictTransportSecurity);
        assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    }
    let error = SecWebSocketExtensionsOwned::builder()
        .extension("permessage-deflate")
        .extension("bad name")
        .build()
        .unwrap_err();
    assert_eq!(error.header(), &FieldName::SecWebSocketExtensions);
    assert_eq!(error.kind(), DecodeErrorKind::InvalidToken);
}

#[test]
fn afternoon_accept_ranges_preserves_custom_unit_wire_and_all_none_spellings() {
    let custom = AcceptRangesOwned::from_units(["xyzzy"]).unwrap();
    assert!(!custom.is_none());
    assert_eq!(custom.to_string(), "xyzzy");
    assert_eq!(custom.units().collect::<Vec<_>>(), ["xyzzy"]);
    for bits in 1_u8..16 {
        let mut spelling = *b"none";
        for (index, byte) in spelling.iter_mut().enumerate() {
            if bits & (1 << index) != 0 {
                *byte = byte.to_ascii_uppercase();
            }
        }
        let wire = std::str::from_utf8(&spelling).unwrap();
        let owned = AcceptRangesOwned::try_from(wire).unwrap();
        assert!(owned.is_none(), "{wire}");
        assert_eq!(owned.to_string(), wire);
        assert_eq!(owned.units().collect::<Vec<_>>(), [wire]);
        let source = RawLine {
            name: AcceptRanges::name(),
            bytes: wire.as_bytes(),
        };
        assert!(AcceptRanges::view(&source).unwrap().unwrap().is_none(), "{wire}");
        assert!(AcceptRanges::owned(&source).unwrap().unwrap().is_none(), "{wire}");
    }
    #[cfg(feature = "serde")]
    for canonical in [AcceptRangesOwned::bytes(), AcceptRangesOwned::none()] {
        let encoded = serde_json::to_value(&canonical).unwrap();
        let restored: AcceptRangesOwned = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.is_none(), canonical.is_none());
        assert_eq!(restored.to_string(), canonical.to_string());
        assert_eq!(restored.units().collect::<Vec<_>>(), canonical.units().collect::<Vec<_>>());
    }
}

#[test]
fn afternoon_origin_authority_classes_preserve_all_labels_and_ipv6_zero_runs() {
    for (wire, expected_host, expected_port) in [
        ("https://a", OriginHost::Domain(OriginDomainView::new("a").unwrap()), None),
        ("ws://a.1", OriginHost::Domain(OriginDomainView::new("a.1").unwrap()), None),
        ("ws://1.2.3.4", OriginHost::Ipv4("1.2.3.4".parse().unwrap()), None),
        ("https://1.2.3.4:7", OriginHost::Ipv4("1.2.3.4".parse().unwrap()), Some(7)),
        (
            "https://[2001:db8:0:1::2:3]",
            OriginHost::Ipv6("2001:db8:0:1:0:0:2:3".parse().unwrap()),
            None,
        ),
    ] {
        let owned = AccessControlAllowOriginOwned::from_origin(wire).unwrap();
        let AccessControlAllowOriginKind::Origin(origin) = owned.kind() else {
            panic!("a serialized tuple origin cannot become wildcard or null");
        };
        assert_eq!(origin.as_str(), wire);
        assert_eq!(origin.host(), expected_host);
        assert_eq!(origin.port(), expected_port);
        assert_eq!(owned.to_string(), wire);
    }
    let address = "2001:db8:0:1:0:0:2:3".parse().unwrap();
    let constructed = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, OriginHost::Ipv6(address), None).unwrap();
    assert_eq!(constructed.to_string(), "https://[2001:db8:0:1::2:3]");
    rejected::<AccessControlAllowOrigin>(&["https://host:11111111111", "ws://-a.b", "ws://11111111111.2.3.4"]);
}

#[test]
fn afternoon_negotiation_empty_members_and_relaxed_weight_spacing_have_distinct_policies() {
    for (wire, strict, relaxed) in [
        (",,gzip,,", true, true),
        ("gzip;q=0.5", true, true),
        ("gzip; q = 0.5", false, true),
        ("gzip;q=\"0.5\"", false, false),
        ("gzip;q = \"0.5\"", false, false),
    ] {
        let source = RawLine {
            name: AcceptEncoding::name(),
            bytes: wire.as_bytes(),
        };
        for (mode, expected) in [(DecodeMode::Strict, strict), (DecodeMode::Relaxed, relaxed)] {
            assert_eq!(AcceptEncoding::view_with(&source, mode).is_ok(), expected, "{wire}/{mode:?}");
            assert_eq!(AcceptEncoding::owned_with(&source, mode).is_ok(), expected, "{wire}/{mode:?}");
        }
    }
    let source = RawLine {
        name: AcceptEncoding::name(),
        bytes: b"gzip;q = \"0.5\"",
    };
    for (mode, expected) in [
        (DecodeMode::Strict, DecodeErrorKind::InvalidSyntax),
        (DecodeMode::Relaxed, DecodeErrorKind::InvalidSyntax),
    ] {
        assert_eq!(AcceptEncoding::view_with(&source, mode).unwrap_err().kind(), expected);
        assert_eq!(AcceptEncoding::owned_with(&source, mode).unwrap_err().kind(), expected);
    }
}
